import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { runInNewContext } from 'node:vm';
const html = await readFile(new URL('../public/index.html', import.meta.url), 'utf8');
const script = await readFile(new URL('../public/site.js', import.meta.url), 'utf8');
// Read actual markup, rather than duplicating its attributes in a fake fixture.
// This deliberately supports only the small DOM surface used by site.js; an
// unexpected selector fails instead of silently masking a production change.
const tags = [...html.matchAll(/<([a-z][\w-]*)\b([^<>]*)>/gi)].map(([, tag, source]) => ({
  tag,
  attributes: Object.fromEntries([...source.matchAll(/([^\s=/'">]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g)]
    .map(([, key, double, single, bare]) => [key, double ?? single ?? bare ?? ''])),
}));
const tagged = predicate => tags.filter(predicate);
function oneTag(predicate) {
  const found = tagged(predicate);
  assert.equal(found.length, 1, 'Expected exactly one matching HTML element');
  return found[0];
}
const byId = id => oneTag(tag => tag.attributes.id === id);
function contentById(id) {
  const { tag } = byId(id);
  const match = html.match(new RegExp(`<${tag}\\b[^>]*\\bid="${id}"[^>]*>([\\s\\S]*?)<\\/${tag}>`));
  assert.ok(match, `Missing content for #${id}`);
  return match[1];
}
const hasClass = (tag, name) => (tag.attributes.class ?? '').split(/\s+/).includes(name);
const systemTags = tagged(tag => Object.hasOwn(tag.attributes, 'data-system'));
const architectureTags = tagged(tag => Object.hasOwn(tag.attributes, 'data-architecture'));
const releaseLinks = html.match(/<div\b[^>]*class="release-links"[^>]*>([\s\S]*?)<\/div>/)?.[1] ?? '';

function createPage(fetchResult) {
  const page = { activeElement: null };
  function element({ tag = 'span', attributes = {} } = {}) {
    const attrs = new Map(Object.entries(attributes));
    const classes = new Set((attributes.class ?? '').split(/\s+/).filter(Boolean));
    const listeners = new Map();
    return {
      tag, children: [], descendants: new Map(),
      dataset: Object.fromEntries(Object.entries(attributes).filter(([key]) => key.startsWith('data-'))
        .map(([key, value]) => [key.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase()), value])),
      disabled: Object.hasOwn(attributes, 'disabled'),
      tabIndex: Number(attributes.tabindex ?? 0),
      textContent: '',
      get href() { return attrs.get('href') ?? ''; },
      set href(value) { attrs.set('href', value); },
      getAttribute: name => attrs.get(name) ?? null,
      setAttribute: (name, value) => attrs.set(name, String(value)),
      classList: {
        contains: name => classes.has(name),
        toggle(name, force = !classes.has(name)) { if (force) classes.add(name); else classes.delete(name); return force; },
      },
      addEventListener(type, handler) { listeners.set(type, [...(listeners.get(type) ?? []), handler]); },
      dispatch(type, details = {}) {
        const event = { target: this, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...details };
        for (const handler of listeners.get(type) ?? []) handler(event);
        return event;
      },
      focus() { page.activeElement = this; },
      querySelector(selector) { assert.ok(this.descendants.has(selector), `Unexpected child selector: ${selector}`); return this.descendants.get(selector); },
      append(...nodes) { this.children = [...this.children.filter(child => !nodes.includes(child)), ...nodes]; },
    };
  }
  const selectors = new Map();
  for (const id of ['hero-download', 'main-nav', 'platform-name', 'download-panel', 'download-github', 'download-gitee', 'channel-note', 'download-selection', 'release-status', 'release-retry', 'architecture-selector', 'mac-chip-help']) {
    selectors.set(`#${id}`, element(byId(id)));
  }
  selectors.set('.menu-toggle', element(oneTag(tag => hasClass(tag, 'menu-toggle'))));
  selectors.set('.release-links', element(oneTag(tag => hasClass(tag, 'release-links'))));
  selectors.set('#platform-icon use', element());
  const tabs = systemTags.map(element);
  const architectures = architectureTags.map(element);
  tabs.forEach(tab => selectors.set(`#${tab.getAttribute('id')}`, tab));
  const query = selector => {
    assert.ok(selectors.has(selector), `Unexpected document selector: ${selector}`);
    return selectors.get(selector);
  };
  const github = query('#download-github');
  const gitee = query('#download-gitee');
  for (const link of [github, gitee]) link.descendants.set('span', element());
  query('.release-links').children = [github, gitee].sort((a, b) =>
    releaseLinks.indexOf(`id="${a.getAttribute('id')}"`) - releaseLinks.indexOf(`id="${b.getAttribute('id')}"`));
  const collections = new Map([['[data-system]', tabs], ['[data-architecture]', architectures]]);
  const document = Object.assign(element(), {
    querySelector: query,
    querySelectorAll(selector) { assert.ok(collections.has(selector), `Unexpected collection: ${selector}`); return collections.get(selector); },
  });
  const forbidden = () => assert.fail('The static website must not call network, storage or desktop APIs');
  const window = { matchMedia: () => ({ addEventListener() {} }), open: forbidden, fetch: forbidden };
  for (const name of ['__TAURI__', 'localStorage', 'sessionStorage']) Object.defineProperty(window, name, { get: forbidden });
  runInNewContext(script, {
    document, window, AbortSignal, fetch: async (url, options) => {
      assert.equal(url, '/api/releases/latest');
      assert.equal(options.cache, 'no-store');
      assert.equal(options.credentials, 'omit');
      return fetchResult();
    }, XMLHttpRequest: forbidden, WebSocket: forbidden,
    EventSource: forbidden, navigator: { sendBeacon: forbidden },
  }, { filename: 'website/public/site.js', timeout: 1_000 });
  return { page, query, tabs, architectures, github, gitee };
}

const targets = ['windows-x64', 'windows-arm64', 'macos-x64', 'macos-arm64', 'linux-x64', 'linux-arm64'];
const data = { version: 'v9.2.1', assets: Object.fromEntries(targets.map(t => [t, { domesticAvailable: t.startsWith('windows'), hkAvailable: t.startsWith('windows') }])) };
const flush = () => new Promise(resolve => setTimeout(resolve, 0));
function verifySelection(view, target) {
  const [system, architecture] = target.split('-');
  assert.equal(view.gitee.href, 'https://downloads.cmmuu.com/download/serylane/latest/' + target);
  assert.equal(view.github.href, '/download/' + target + '?channel=github');
  assert.deepEqual(view.query('.release-links').children, [view.gitee, view.github]);
  assert.equal(view.gitee.classList.contains('button-primary'), true);
  assert.equal(view.github.classList.contains('button-secondary'), true);
  assert.equal(view.query('#download-panel').getAttribute('aria-labelledby'), 'tab-' + system);
  for (const tab of view.tabs) {
    assert.equal(tab.disabled, false);
    assert.equal(tab.getAttribute('aria-selected'), String(tab.dataset.system === system));
    assert.equal(tab.tabIndex, tab.dataset.system === system ? 0 : -1);
  }
  for (const button of view.architectures) assert.equal(button.getAttribute('aria-pressed'), String(button.dataset.architecture === architecture));
  assert.equal(view.query('#mac-chip-help').hidden, system !== 'macos');
  for (const button of view.architectures) {
    const label = system === 'macos'
      ? button.dataset.architecture === 'arm64' ? 'Apple 芯片（M 系列）' : 'Intel 芯片'
      : button.dataset.architecture === 'arm64' ? 'ARM64' : 'x64';
    assert.equal(button.textContent, label);
    if (button.dataset.architecture === architecture) {
      assert.ok(view.gitee.getAttribute('aria-label').includes(label));
      assert.ok(view.query('#download-selection').textContent.includes(label));
    }
  }
  for (const link of [view.github, view.gitee]) {
    assert.equal(link.getAttribute('target'), '_blank');
    assert.match(link.getAttribute('rel'), /noopener/);
    assert.match(link.getAttribute('aria-label'), /点击时重新核对版本/);
  }
}
assert.equal(byId('download-gitee').attributes.href, 'https://downloads.cmmuu.com/download/serylane/latest/windows-x64');
assert.equal(byId('download-github').attributes.href, '/download/windows-x64?channel=github');
for (const tag of [...systemTags, ...architectureTags]) assert.ok(Object.hasOwn(tag.attributes, 'disabled'));
assert.doesNotMatch(html, /releases\/download\/v[0-9]/, 'No version-pinned HTML fallback');
assert.match(html, /界面示意 · 非实时状态/);
assert.match(html, /三个独立操作/);
assert.equal(byId('hero-download').attributes.href, '#download');
assert.equal(contentById('hero-download').replace(/<[^>]+>/g, ''), '下载 Serylane');
assert.match(html, /支持 Windows、macOS 与 Linux/);
assert.doesNotMatch(html, /data-select-windows|下载 Windows 版/);
let complete;
const view = createPage(() => new Promise(resolve => { complete = resolve; }));
assert.match(view.query('#release-status').textContent, /正在查询/);
verifySelection(view, 'windows-x64');
view.tabs[1].dispatch('click');
verifySelection(view, 'macos-arm64');
complete({ ok: true, json: async () => data });
await flush();
verifySelection(view, 'macos-arm64');
assert.match(view.query('#release-status').textContent, /v9.2.1/);
for (const target of targets) {
  const [system, arch] = target.split('-');
  view.tabs.find(t => t.dataset.system === system).dispatch('click');
  view.architectures.find(a => a.dataset.architecture === arch).dispatch('click');
  verifySelection(view, target);
  assert.match(view.query('#channel-note').textContent, system === 'windows' ? /已核对下载中心/ : /暂未确认下载中心/);
}
view.tabs[0].dispatch('keydown', { key: 'End' });
verifySelection(view, 'linux-x64');
assert.equal(view.page.activeElement, view.tabs[2]);
view.tabs[2].dispatch('keydown', { key: 'ArrowRight' });
verifySelection(view, 'windows-x64');
view.tabs[0].dispatch('keydown', { key: 'ArrowLeft' });
verifySelection(view, 'linux-x64');
view.tabs[2].dispatch('keydown', { key: 'Home' });
verifySelection(view, 'windows-x64');
// The neutral hero link scrolls to the selector without resetting a user's
// platform or chip selection to Windows. Its fragment also works without JS.
for (const target of targets) {
  const [system, arch] = target.split('-');
  view.tabs.find(t => t.dataset.system === system).dispatch('click');
  view.architectures.find(a => a.dataset.architecture === arch).dispatch('click');
  assert.equal(view.query('#hero-download').dispatch('click').defaultPrevented, false);
  verifySelection(view, target);
}
for (const failure of ['network', 'status', 'malformed', 'incomplete']) {
  let recovered = false;
  const errorView = createPage(async () => {
    if (recovered) return { ok: true, json: async () => data };
    if (failure === 'network') throw new Error('offline');
    if (failure === 'status') return { ok: false };
    if (failure === 'malformed') return { ok: true, json: async () => { throw new Error('bad json'); } };
    return { ok: true, json: async () => ({ version: 'v9.2.1', assets: {} }) };
  });
  await flush();
  assert.match(errorView.query('#release-status').textContent, /无法确认/);
  assert.doesNotMatch(errorView.query('#release-status').textContent, /当前最新正式版/);
  assert.equal(errorView.query('#release-retry').hidden, false);
  verifySelection(errorView, 'windows-x64');
  recovered = true;
  errorView.query('#release-retry').dispatch('click');
  await flush();
  assert.match(errorView.query('#release-status').textContent, /v9.2.1/);
  assert.equal(errorView.query('#release-retry').hidden, true);
}
console.log('PASS: six platforms, keyboard, loading/errors/retry, no-JS fallback, independent click-time URLs.');
