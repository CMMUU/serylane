import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { runInNewContext } from 'node:vm';

const script = await readFile(new URL('../public/site.js', import.meta.url), 'utf8');
const targets = ['windows-x64', 'windows-arm64', 'macos-x64', 'macos-arm64', 'linux-x64', 'linux-arm64'];
const suffixes = ['x64-setup.exe', 'arm64-setup.exe', 'x64.dmg', 'aarch64.dmg', 'amd64.AppImage', 'aarch64.AppImage'];
const catalog = { version: 'v9.2.1', assets: Object.fromEntries(targets.map(target => [target, { channel: 'hk' }])) };
const success = data => ({ ok: true, json: async () => data });
const flush = () => new Promise(resolve => setImmediate(resolve));
function download(target, version = 'v9.2.1') {
  const [major, minor, patch] = version.slice(1).split('.').map(Number);
  const brand = major === 0 && (minor < 7 || (minor === 7 && patch < 7)) ? 'RouteDeck' : 'Serylane';
  return { version, channel: 'hk', url: `https://files.cmmuu.com/releases/serylane/${version}/${brand}_${version.slice(1)}_${suffixes[targets.indexOf(target)]}` };
}

// Parse the actual bilingual pages, not substitute HTML fixtures. An unexpected
// script selector fails explicitly rather than being hidden by a permissive DOM.
function markup(html) {
  const tags = [...html.matchAll(/<([a-z][\w-]*)\b((?:"[^"]*"|'[^']*'|[^'">])*)>/gi)].map(match => ({
    tag: match[1].toLowerCase(), start: match.index + match[0].length,
    attributes: Object.fromEntries([...match[2].matchAll(/([^\s=/'">]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g)]
      .map(([, key, double, single, bare]) => [key, double ?? single ?? bare ?? ''])),
  }));
  const tagged = predicate => tags.filter(predicate);
  const one = predicate => {
    const found = tagged(predicate);
    assert.equal(found.length, 1, 'Expected exactly one matching HTML element');
    return found[0];
  };
  const byId = id => one(tag => tag.attributes.id === id);
  const hasClass = (tag, name) => (tag.attributes.class ?? '').split(/\s+/).includes(name);
  const content = tag => html.slice(tag.start, html.indexOf(`</${tag.tag}>`, tag.start)).replace(/<[^>]*>/g, '').replace(/\s+/g, ' ').trim();
  return { tags, tagged, one, byId, hasClass, content };
}

function createPage(source, fetchResult = async () => success(catalog), resolveResult = async url => success(download(url.split('/').at(-1))), options = {}) {
  const page = { activeElement: null, navigations: [], requests: [] };
  const selectors = new Map();
  function element({ tag = 'span', attributes = {}, start } = {}) {
    const attrs = new Map(Object.entries(attributes));
    const classes = new Set((attributes.class ?? '').split(/\s+/).filter(Boolean));
    const listeners = new Map();
    return {
      tag,
      dataset: Object.fromEntries(Object.entries(attributes).filter(([key]) => key.startsWith('data-'))
        .map(([key, value]) => [key.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase()), value])),
      lang: attributes.lang ?? '',
      hidden: Object.hasOwn(attributes, 'hidden'),
      disabled: Object.hasOwn(attributes, 'disabled'),
      textContent: start === undefined ? '' : source.content({ tag, start }),
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
        const event = { target: this, button: 0, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...details };
        for (const handler of listeners.get(type) ?? []) handler(event);
        return event;
      },
      closest(selector) {
        if (selector === 'a') return tag === 'a' ? this : null;
        if (selector === '.site-header') return this.withinHeader || classes.has('site-header') ? selectors.get('.site-header') : null;
        assert.fail(`Unexpected closest selector: ${selector}`);
      },
      focus() { page.activeElement = this; },
    };
  }
  for (const id of ['hero-download', 'main-nav', 'download-progress', 'release-status', 'release-retry', 'download-selection']) {
    const found = source.tagged(tag => tag.attributes.id === id);
    selectors.set(`#${id}`, found.length ? element(found[0]) : null);
  }
  selectors.set('.menu-toggle', element(source.one(tag => source.hasClass(tag, 'menu-toggle'))));
  selectors.set('.site-header', element(source.one(tag => source.hasClass(tag, 'site-header'))));
  const links = options.noDownloads ? [] : source.tagged(tag => source.hasClass(tag, 'download-target') && Object.hasOwn(tag.attributes, 'data-download-target')).map(element);
  if (options.noDownloads) for (const id of ['hero-download', 'download-progress', 'release-status', 'release-retry', 'download-selection']) selectors.set(`#${id}`, null);
  if (options.noSelection) selectors.set('#download-selection', null);
  const query = selector => {
    assert.ok(selectors.has(selector), `Unexpected document selector: ${selector}`);
    return selectors.get(selector);
  };
  const document = Object.assign(element(), {
    documentElement: element(source.one(tag => tag.tag === 'html')),
    querySelector: query,
    querySelectorAll(selector) { assert.equal(selector, '.download-target[data-download-target]'); return links; },
  });
  const forbidden = () => assert.fail('site.js must not call network, storage or desktop APIs outside the two public release APIs');
  Object.defineProperty(document, 'cookie', { get: forbidden, set: forbidden });
  const wide = { listeners: [], addEventListener(type, handler) { assert.equal(type, 'change'); this.listeners.push(handler); }, change(matches) { this.listeners.forEach(handler => handler({ matches })); } };
  const window = {
    matchMedia(query) { assert.equal(query, '(min-width: 641px)'); return wide; },
    open: forbidden, fetch: forbidden, location: { assign: url => page.navigations.push(url) },
  };
  for (const name of ['__TAURI__', 'localStorage', 'sessionStorage']) Object.defineProperty(window, name, { get: forbidden });
  const sandbox = {
    document, window, AbortSignal, requestAnimationFrame: callback => callback(), fetch: async (url, init) => {
      page.requests.push(url);
      assert.ok(url === '/api/releases/latest' || /^\/api\/download\/(windows|macos|linux)-(x64|arm64)$/.test(url), `Forbidden request: ${url}`);
      assert.equal(init.method ?? 'GET', 'GET');
      assert.equal(init.cache, 'no-store');
      assert.equal(init.credentials, 'omit');
      assert.ok(init.signal instanceof AbortSignal);
      return url === '/api/releases/latest' ? fetchResult() : resolveResult(url);
    }, XMLHttpRequest: forbidden, WebSocket: forbidden, EventSource: forbidden,
    navigator: { sendBeacon: forbidden },
  };
  for (const name of ['localStorage', 'sessionStorage']) Object.defineProperty(sandbox, name, { get: forbidden });
  runInNewContext(script, sandbox, { filename: 'website/public/site.js', timeout: 1_000 });
  return { page, query, links, document, wide, link: target => links.find(link => link.dataset.downloadTarget === target) };
}

for (const [path, language] of [['../public/index.html', 'en'], ['../public/zh/index.html', 'zh-CN']]) {
  const html = await readFile(new URL(path, import.meta.url), 'utf8');
  const source = markup(html);
  const chinese = language === 'zh-CN';
  const messages = chinese
    ? { loading: /正在查询/, latest: /当前最新正式版/, unavailable: /无法确认/, retry: /重试/, started: /已交给浏览器/, checking: /正在核对/ }
    : { loading: /Checking the latest/, latest: /Latest stable release:/, unavailable: /could not be verified/, retry: /retry/i, started: /Handed to your browser/, checking: /Checking the download center/ };
  assert.equal(source.one(tag => tag.tag === 'html').attributes.lang, language);
  assert.equal(source.byId('hero-download').attributes.href, '#download');
  assert.equal(source.content(source.byId('hero-download')), chinese ? '下载 Serylane' : 'Download Serylane');
  assert.equal(source.byId('release-status').attributes.role, 'status');
  assert.equal(source.byId('download-progress').attributes.role, 'status');
  assert.equal(source.byId('download-progress').attributes['aria-live'], 'polite');
  assert.ok(Object.hasOwn(source.byId('release-retry').attributes, 'hidden'));
  assert.doesNotMatch(html, /download-github|download-gitee|channel=|GitHub 备用|data-select-windows|releases\/download\/v[0-9]/i);
  assert.doesNotMatch(html, /\bdata-(system|architecture)=/, 'Old JS-only tab selectors must not replace the six real anchors');
  const linkTags = source.tagged(tag => source.hasClass(tag, 'download-target'));
  assert.equal(linkTags.length, 6, 'All six downloads are visible anchors, including without JavaScript');
  for (const target of targets) {
    const tag = source.one(tag => tag.attributes['data-download-target'] === target);
    const label = source.content(tag);
    const format = target.startsWith('windows-') ? 'EXE' : target.startsWith('macos-') ? 'DMG' : 'AppImage';
    const chip = target === 'macos-arm64' ? chinese ? 'Apple 芯片（M 系列）' : 'Apple silicon (M series)'
      : target === 'macos-x64' ? chinese ? 'Intel 芯片' : 'Intel' : target.endsWith('-arm64') ? 'ARM64' : 'x64';
    assert.equal(tag.tag, 'a');
    assert.equal(tag.attributes.href, '/download/' + target);
    assert.equal(tag.attributes.target, '_blank');
    assert.ok((tag.attributes.rel ?? '').split(/\s+/).includes('noopener'));
    assert.ok(tag.attributes.rel.split(/\s+/).includes('noreferrer'));
    assert.ok(label.includes(chip), `${target}: fixed, distinguishable architecture label`);
    assert.ok(tag.attributes['aria-label'].includes(chip));
    assert.ok(tag.attributes['aria-label'].includes(format));
    assert.ok(tag.attributes['aria-label'].includes('Serylane'));
    assert.match(tag.attributes['aria-label'], chinese ? /下载.*最新|最新.*下载/ : /download.*latest|latest.*download/i);
    assert.equal(Object.hasOwn(tag.attributes, 'disabled'), false);
  }
  function verifyAnchors(view) {
    for (const target of targets) {
      const link = view.link(target);
      const original = linkTags.find(tag => tag.attributes['data-download-target'] === target);
      assert.equal(link.href, '/download/' + target);
      assert.equal(link.textContent, source.content(original), 'Availability must never rewrite architecture labels');
      assert.equal(link.getAttribute('aria-label'), original.attributes['aria-label']);
      assert.equal(link.disabled, false, 'An unavailable catalog still permits a fresh click-time check');
      assert.notEqual(link.getAttribute('aria-disabled'), 'true');
    }
  }

  let complete;
  const view = createPage(source, () => new Promise(resolve => { complete = resolve; }));
  assert.match(view.query('#release-status').textContent, messages.loading);
  assert.equal(view.query('#release-retry').disabled, true);
  verifyAnchors(view);
  view.query('#release-retry').dispatch('click');
  assert.deepEqual(view.page.requests, ['/api/releases/latest'], 'Duplicate pending catalog checks are deduplicated');
  complete(success(catalog));
  await flush();
  assert.match(view.query('#release-status').textContent, /v9\.2\.1/);
  assert.equal(view.query('#release-retry').hidden, true);
  verifyAnchors(view);
  assert.equal(view.query('#hero-download').dispatch('click').defaultPrevented, false);
  assert.equal(view.page.activeElement, null, 'Download initialization does not steal focus');

  // Preserve the navigation's keyboard/focus behavior on both locales and docs.
  for (const noDownloads of [false, true]) {
    const menuView = createPage(source, undefined, undefined, { noDownloads });
    await flush();
    const menuButton = menuView.query('.menu-toggle');
    menuButton.dispatch('click');
    assert.equal(menuButton.getAttribute('aria-expanded'), 'true');
    assert.equal(menuButton.getAttribute('aria-label'), chinese ? '关闭导航' : 'Close navigation');
    assert.equal(menuView.query('#main-nav').classList.contains('is-open'), true);
    menuView.document.dispatch('keydown', { key: 'Escape' });
    assert.equal(menuButton.getAttribute('aria-expanded'), 'false');
    assert.equal(menuButton.getAttribute('aria-label'), chinese ? '打开导航' : 'Open navigation');
    assert.equal(menuView.page.activeElement, menuButton);
    menuButton.dispatch('click');
    menuView.wide.change(true);
    assert.equal(menuButton.getAttribute('aria-expanded'), 'false');
    menuButton.dispatch('click');
    menuView.document.dispatch('click');
    assert.equal(menuButton.getAttribute('aria-expanded'), 'false');
    if (noDownloads) assert.deepEqual(menuView.page.requests, [], 'Docs/menu-only pages never request releases');
  }

  // Modified/middle/right clicks preserve native anchor handling and make no API request.
  for (const target of targets) for (const detail of [{ ctrlKey: true }, { metaKey: true }, { shiftKey: true }, { altKey: true }, { button: 1 }, { button: 2 }, { defaultPrevented: true }]) {
    const count = view.page.requests.length;
    const event = view.link(target).dispatch('click', detail);
    assert.equal(event.defaultPrevented, !!detail.defaultPrevented);
    assert.equal(view.page.requests.length, count);
    verifyAnchors(view);
  }

  for (const failure of ['network', 'status', 'malformed', 'incomplete', 'null', 'version', 'prerelease', 'legacy-github', 'legacy-gitee']) {
    let recovered = false;
    const errorView = createPage(source, async () => {
      if (recovered) return success(catalog);
      if (failure === 'network') throw new Error('offline');
      if (failure === 'status') return { ok: false };
      if (failure === 'malformed') return { ok: true, json: async () => { throw new Error('bad json'); } };
      if (failure === 'incomplete') return success({ version: 'v9.2.1', assets: {} });
      if (failure === 'null') return success(null);
      if (failure === 'version') return success({ ...catalog, version: 'v09.2.1' });
      if (failure === 'prerelease') return success({ ...catalog, version: 'v9.2.1-beta.1' });
      return success({ ...catalog, assets: { ...catalog.assets, 'linux-arm64': { channel: failure.slice(7) } } });
    });
    await flush();
    assert.match(errorView.query('#release-status').textContent, messages.unavailable, failure);
    assert.doesNotMatch(errorView.query('#release-status').textContent, messages.latest);
    assert.equal(errorView.query('#release-retry').hidden, false);
    assert.equal(errorView.query('#release-retry').disabled, false);
    verifyAnchors(errorView);
    // Catalog failure is not a permanent disabled state: the click resolves afresh.
    errorView.link('windows-x64').dispatch('click');
    await flush();
    assert.deepEqual(errorView.page.navigations, [download('windows-x64').url]);
    recovered = true;
    errorView.query('#release-retry').dispatch('click');
    await flush();
    assert.match(errorView.query('#release-status').textContent, /v9\.2\.1/);
    assert.equal(errorView.query('#release-retry').hidden, true);
  }

  // All six click-time requests accept only their own exact current asset, even
  // when the catalog shown on an already open page names a previous version.
  for (const target of targets) {
    const current = download(target, 'v9.2.2');
    const downloadView = createPage(source, undefined, async () => success(current), { noSelection: true });
    await flush();
    assert.match(downloadView.query('#release-status').textContent, /v9\.2\.1/);
    assert.equal(downloadView.link(target).dispatch('click').defaultPrevented, true);
    await flush();
    assert.deepEqual(downloadView.page.navigations, [current.url]);
    assert.equal(downloadView.page.requests.at(-1), '/api/download/' + target);
    assert.match(downloadView.query('#download-progress').textContent, messages.started);
    assert.ok(downloadView.query('#download-progress').textContent.includes('v9.2.2'));
    assert.ok(downloadView.query('#download-progress').textContent.includes(target));
    verifyAnchors(downloadView);
    assert.equal(downloadView.page.activeElement, null);
  }

  const valid = download('windows-x64');
  const invalid = [
    { channel: 'github' }, { channel: 'gitee' }, { channel: undefined },
    { url: 'https://github.com/CMMUU/serylane/releases/download/v9.2.1/package.exe' },
    { url: 'https://gitee.com/CMMUU/serylane/releases/download/v9.2.1/package.exe' },
    { url: valid.url + '?fallback=github' }, { url: valid.url + '#fragment' },
    { url: valid.url.replace('x64', 'arm64') }, { version: 'v9.2.2' },
    { version: 'v09.2.1' }, { version: 'v9.02.1' }, { version: 'v9.2.01' },
    { version: 'v9.2.1-beta.1' }, { version: 'v9.2.1+build' }, { version: null },
    { url: valid.url.replace('files.cmmuu.com', 'files.cmmuu.com.evil.example') },
    { url: valid.url.replace('files.cmmuu.com', 'user:password@files.cmmuu.com') },
    { url: valid.url.replace('https:', 'http:') }, { url: valid.url.replace('https:', '') },
    { url: valid.url.replace('files.cmmuu.com', '127.0.0.1:7890') },
    { url: valid.url.replace('files.cmmuu.com', 'localhost:7890') },
    { url: valid.url.replace('Serylane_', 'RouteDeck_') },
    { url: valid.url.replace('/v9.2.1/', '/v9.2.0/') },
  ];
  for (const change of invalid) {
    const invalidView = createPage(source, undefined, async () => success({ ...valid, ...change }));
    await flush();
    invalidView.link('windows-x64').dispatch('click');
    await flush();
    assert.deepEqual(invalidView.page.navigations, [], JSON.stringify(change));
    assert.match(invalidView.query('#download-progress').textContent, messages.retry);
    assert.doesNotMatch(invalidView.query('#download-progress').textContent, /备用|fallback|GitHub|Gitee/i);
    verifyAnchors(invalidView);
  }
  for (const target of targets) {
    const wrong = download(targets[(targets.indexOf(target) + 1) % targets.length]);
    const invalidView = createPage(source, undefined, async () => success(wrong));
    await flush();
    invalidView.link(target).dispatch('click');
    await flush();
    assert.deepEqual(invalidView.page.navigations, [], `${target}: never substitute another architecture`);
  }
  for (const version of ['v0.6.9', 'v0.7.6', 'v0.7.7', 'v0.8.0', 'v1.0.0']) {
    const asset = download('windows-x64', version);
    const historicalView = createPage(source, undefined, async () => success(asset));
    await flush();
    historicalView.link('windows-x64').dispatch('click');
    await flush();
    assert.deepEqual(historicalView.page.navigations, [asset.url], 'Real historical filename identity must be retained');
    const wrongBrand = { ...asset, url: asset.url.replace(version === 'v0.6.9' || version === 'v0.7.6' ? 'RouteDeck_' : 'Serylane_', version === 'v0.6.9' || version === 'v0.7.6' ? 'Serylane_' : 'RouteDeck_') };
    const wrongView = createPage(source, undefined, async () => success(wrongBrand));
    await flush();
    wrongView.link('windows-x64').dispatch('click');
    await flush();
    assert.deepEqual(wrongView.page.navigations, [], 'Historical names must not be rewritten for display branding');
  }
  for (const failure of ['network', 'status', 'malformed', 'null']) {
    let available = false;
    const retryView = createPage(source, undefined, async () => {
      if (available) return success(valid);
      if (failure === 'network') throw new Error('offline');
      if (failure === 'status') return { ok: false };
      if (failure === 'null') return success(null);
      return { ok: true, json: async () => { throw new Error('invalid json'); } };
    });
    await flush();
    retryView.link('windows-x64').dispatch('click');
    await flush();
    assert.deepEqual(retryView.page.navigations, []);
    assert.match(retryView.query('#download-progress').textContent, messages.retry);
    available = true;
    retryView.link('windows-x64').dispatch('click');
    await flush();
    assert.deepEqual(retryView.page.navigations, [valid.url]);
  }
  let resolvePending;
  const pendingView = createPage(source, undefined, () => new Promise(resolve => { resolvePending = resolve; }));
  await flush();
  pendingView.link('windows-x64').dispatch('click');
  assert.match(pendingView.query('#download-progress').textContent, messages.checking);
  pendingView.link('windows-x64').dispatch('click');
  assert.deepEqual(pendingView.page.requests, ['/api/releases/latest', '/api/download/windows-x64']);
  resolvePending(success(valid));
  await flush();
  assert.deepEqual(pendingView.page.navigations, [valid.url]);
  console.log(`PASS: ${language} real HTML — six native anchors, localized status/menu, no-JS/modified clicks, loading/errors/retry, deduplication, exact click-time version/brand/host/channel/architecture and no forbidden APIs.`);
}
