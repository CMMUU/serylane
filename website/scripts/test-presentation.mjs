import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { runInNewContext } from 'node:vm';

const themeScript = await readFile(new URL('../public/theme.js', import.meta.url), 'utf8');
const siteScript = await readFile(new URL('../public/site.js', import.meta.url), 'utf8');
const key = 'serylane-theme';
const preferences = ['dark', 'light', 'system'];
const colors = { dark: '#0a0d14', light: '#f6f8fc' };
const pages = await Promise.all(['index.html', 'zh/index.html', 'docs/index.html', 'docs/routing.html', 'docs/verification.html', 'docs/privacy.html', 'zh/docs/index.html', 'zh/docs/routing.html', 'zh/docs/verification.html', 'zh/docs/privacy.html', '404.html'].map(async path => {
  const html = await readFile(new URL('../public/' + path, import.meta.url), 'utf8');
  const tags = [...html.matchAll(/<([a-z][\w-]*)\b((?:"[^"]*"|'[^']*'|[^'">])*)>/gi)].map(match => ({
    tag: match[1].toLowerCase(), start: match.index + match[0].length,
    attributes: Object.fromEntries([...match[2].matchAll(/([^\s=/'">]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g)]
      .map(([, name, double, single, bare]) => [name, double ?? single ?? bare ?? ''])),
  }));
  const one = predicate => {
    const found = tags.filter(predicate);
    assert.equal(found.length, 1, path + ': expected exactly one matching tag');
    return found[0];
  };
  const inner = tag => html.slice(tag.start, html.indexOf(`</${tag.tag}>`, tag.start));
  const hasClass = (tag, name) => (tag.attributes.class ?? '').split(/\s+/).includes(name);
  return { path, html, tags, one, inner, hasClass };
}));

// A strict, event-driven DOM/MediaQuery/IntersectionObserver VM. It exposes no
// application hooks; tests use the same controls and events as a real browser.
function createPage(source, options = {}) {
  const page = { reads: [], writes: [], forbidden: [], observers: [], activeElement: null, focusCalls: 0, storage: options.storage ?? { value: options.stored ?? null } };
  const expectedWrites = [];
  const forbid = message => {
    page.forbidden.push(message);
    assert.fail(message);
  };
  function element({ tag = 'span', attributes = {} } = {}) {
    const attrs = new Map(Object.entries(attributes));
    const classes = new Set((attributes.class ?? '').split(/\s+/));
    const listeners = new Map();
    return {
      tag, lang: attributes.lang ?? '', disabled: Object.hasOwn(attributes, 'disabled'),
      dataset: Object.fromEntries(Object.entries(attributes).filter(([name]) => name.startsWith('data-'))
        .map(([name, value]) => [name.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase()), value])),
      textContent: '',
      get href() { return attrs.get('href') ?? ''; },
      set href(value) { attrs.set('href', value); },
      getAttribute: name => attrs.get(name) ?? null,
      setAttribute: (name, value) => attrs.set(name, String(value)),
      classList: {
        contains: name => classes.has(name),
        toggle(name, force = !classes.has(name)) { if (force) classes.add(name); else classes.delete(name); return force; },
      },
      addEventListener(type, handler, options = {}) {
        if (Object.hasOwn(attributes, 'data-theme-value') && /^key/.test(type)) return forbid('Theme buttons use native keyboard activation');
        listeners.set(type, [...(listeners.get(type) ?? []), { handler, once: options.once }]);
      },
      listenerTypes: () => [...listeners.keys()],
      get innerHTML() { return ''; },
      set innerHTML(value) { forbid('Controls must preserve their DOM identity and icons'); },
      set outerHTML(value) { forbid('Controls must preserve their DOM identity and icons'); },
      dispatch(type, details = {}) {
        const event = { target: this, button: 0, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...details };
        const current = listeners.get(type) ?? [];
        listeners.set(type, current.filter(item => !item.once));
        for (const { handler } of current) handler(event);
        return event;
      },
      closest(selector) {
        if (selector === 'a') return tag === 'a' ? this : null;
        if (selector === '.site-header') return classes.has('site-header') ? this : null;
        return forbid('Unexpected closest selector: ' + selector);
      },
      focus() { page.activeElement = this; page.focusCalls++; },
    };
  }
  const root = element(source.one(tag => tag.tag === 'html'));
  const meta = element(source.one(tag => tag.tag === 'meta' && tag.attributes.name === 'theme-color'));
  const themeTags = source.tags.filter(tag => tag.tag === 'button' && Object.hasOwn(tag.attributes, 'data-theme-value'));
  const themeGroups = [themeTags.map(element)];
  if (options.duplicateThemeGroup) themeGroups.push(themeTags.map(element));
  const themeButtons = themeGroups.flat();
  const diagramTag = source.tags.find(tag => tag.tag === 'figure' && Object.hasOwn(tag.attributes, 'data-routing-diagram'));
  const diagram = diagramTag ? element(diagramTag) : null;
  const buttonTag = source.tags.find(tag => tag.tag === 'button' && Object.hasOwn(tag.attributes, 'data-motion-toggle'));
  const button = options.noMotionButton || !buttonTag ? null : element(buttonTag);
  const menuButton = element(source.one(tag => source.hasClass(tag, 'menu-toggle')));
  const menu = element(source.one(tag => tag.attributes.id === 'main-nav'));
  const header = element(source.one(tag => source.hasClass(tag, 'site-header')));
  const downloads = source.tags.filter(tag => source.hasClass(tag, 'download-target')).map(element);
  const selectors = new Map([
    ['meta[name="theme-color"]', meta], ['figure[data-routing-diagram]', diagram], ['button[data-motion-toggle]', button],
    ['.menu-toggle', menuButton], ['#main-nav', menu], ['.site-header', header],
  ]);
  const document = Object.assign(element(), {
    documentElement: root, readyState: options.readyState ?? 'loading', hidden: options.hidden ?? false,
    querySelector(selector) {
      if (this.readyState === 'loading' && selector !== 'meta[name="theme-color"]') return forbid('Controls must wait for DOMContentLoaded: ' + selector);
      if (!selectors.has(selector)) return forbid('Unexpected document selector: ' + selector);
      return selectors.get(selector);
    },
    querySelectorAll(selector) {
      if (this.readyState === 'loading') return forbid('Control collections must wait for DOMContentLoaded');
      if (selector === 'button[data-theme-value]') return themeButtons;
      if (selector === '.download-target[data-download-target]') return downloads;
      return forbid('Unexpected collection: ' + selector);
    },
  });
  Object.defineProperty(document, 'cookie', { get: () => forbid('No cookies'), set: () => forbid('No cookies') });
  function media(matches) {
    const handlers = [];
    return {
      matches,
      addEventListener(type, handler) { assert.equal(type, 'change'); handlers.push(handler); },
      change(matches) { this.matches = matches; for (const handler of handlers) handler({ matches }); },
    };
  }
  const scheme = media(options.systemDark ?? false);
  const reduced = media(options.reducedMotion ?? false);
  const wide = media(false);
  const storage = new Proxy({
    getItem(name) {
      page.reads.push(name);
      if (name !== key) return forbid('Forbidden storage read: ' + name);
      if (options.throwGet) throw new Error('Storage getter denied');
      return page.storage.value;
    },
    setItem(name, value) {
      page.writes.push([name, value]);
      if (name !== key || !preferences.includes(value)) return forbid('Forbidden storage write');
      if (options.throwSet) throw new Error('Storage setter denied');
      page.storage.value = value;
    },
  }, {
    get(target, property) { if (!Object.hasOwn(target, property)) return forbid('Forbidden storage operation: ' + String(property)); return target[property]; },
    set: () => forbid('Only the one-key storage methods are allowed'),
    ownKeys: () => forbid('No storage enumeration'),
  });
  const window = {
    fetch: () => forbid('No theme/doc requests'), open: () => forbid('No popup'),
    location: { assign: () => forbid('No theme/doc navigation') },
  };
  if (!options.noMatchMedia) window.matchMedia = query => {
    if (query === '(prefers-color-scheme: dark)') return scheme;
    if (query === '(prefers-reduced-motion: reduce)') return reduced;
    if (query === '(min-width: 641px)') return wide;
    return forbid('Unexpected media query: ' + query);
  };
  if (!options.noObserver) window.IntersectionObserver = class {
    constructor(callback) { this.callback = callback; this.targets = []; page.observers.push(this); }
    observe(target) { this.targets.push(target); }
    emit(isIntersecting) { this.callback([{ target: diagram, isIntersecting }]); }
  };
  Object.defineProperty(window, 'localStorage', { get() { if (options.throwStorageProperty) throw new Error('Storage property denied'); return storage; } });
  for (const name of ['sessionStorage', '__TAURI__']) Object.defineProperty(window, name, { get: () => forbid('Forbidden window property: ' + name) });
  const sandbox = {
    document, window, fetch: () => forbid('No theme/doc requests'),
    XMLHttpRequest: () => forbid('No XHR'), WebSocket: () => forbid('No WebSocket'), EventSource: () => forbid('No event stream'),
    navigator: { sendBeacon: () => forbid('No beacon') },
  };
  Object.defineProperty(sandbox, 'localStorage', { get: () => storage });
  Object.defineProperty(sandbox, 'sessionStorage', { get: () => forbid('No session storage') });
  const originalGlobals = Object.getOwnPropertyNames(sandbox).sort();
  const originalWindowGlobals = Object.getOwnPropertyNames(window).sort();
  runInNewContext(themeScript, sandbox, { filename: 'website/public/theme.js', timeout: 1_000 });
  function safe() {
    assert.deepEqual(page.forbidden, []);
    assert.deepEqual(page.reads, options.throwStorageProperty ? [] : [key], 'Only one initial read of the permitted key');
    assert.ok(page.writes.every(([name, value]) => name === key && preferences.includes(value)));
    assert.deepEqual(page.writes, expectedWrites, 'Exactly one write per valid button click and no incidental storage writes');
    assert.deepEqual(Object.getOwnPropertyNames(sandbox).sort(), originalGlobals, 'Scripts must not export global hooks');
    assert.deepEqual(Object.getOwnPropertyNames(window).sort(), originalWindowGlobals, 'Scripts must not export window hooks');
  }
  safe();
  return Object.assign(page, {
    root, meta, themeGroups, themeButtons, diagram, button, menuButton, menu, document, scheme, reduced, downloads,
    safe,
    ready() { document.readyState = 'interactive'; document.dispatch('DOMContentLoaded'); safe(); },
    setTheme(value, index = 0) {
      const control = themeGroups[index].find(button => button.dataset.themeValue === value);
      assert.ok(control, 'A real theme button must exist for ' + value);
      if (!options.throwStorageProperty) expectedWrites.push([key, value]);
      const event = control.dispatch('click');
      assert.equal(event.defaultPrevented, false, 'Theme buttons keep native activation');
      safe();
      return control;
    },
    hide(hidden) { document.hidden = hidden; document.dispatch('visibilitychange'); safe(); },
    runSite() { runInNewContext(siteScript, sandbox, { filename: 'website/public/site.js', timeout: 1_000 }); safe(); },
  });
}
function assertTheme(page, theme, preference) {
  assert.equal(page.root.dataset.theme, theme);
  assert.equal(page.meta.getAttribute('content'), colors[theme]);
  if (preference) for (const group of page.themeGroups) {
    assert.equal(group.filter(button => button.getAttribute('aria-pressed') === 'true').length, 1, 'Exactly one selected preference per group');
    for (const button of group) assert.equal(button.getAttribute('aria-pressed'), String(button.dataset.themeValue === preference));
  }
  page.safe();
}

for (const source of pages) {
  const htmlTag = source.one(tag => tag.tag === 'html');
  assert.ok(['en', 'zh-CN'].includes(htmlTag.attributes.lang), source.path);
  assert.equal(htmlTag.attributes['data-theme'], 'dark', source.path + ': dark no-JS default');
  const themeTag = source.one(tag => tag.tag === 'script' && tag.attributes.src === '/theme.js');
  const siteTag = source.one(tag => tag.tag === 'script' && tag.attributes.src === '/site.js');
  assert.ok(themeTag.start < source.html.indexOf('</head>'));
  assert.ok(themeTag.start < siteTag.start);
  assert.equal(Object.hasOwn(themeTag.attributes, 'async'), false, 'Theme runs synchronously before first paint');
  assert.equal(Object.hasOwn(themeTag.attributes, 'defer'), false);
  assert.ok(Object.hasOwn(siteTag.attributes, 'defer'));
  const chinese = htmlTag.attributes.lang === 'zh-CN';
  const group = source.one(tag => source.hasClass(tag, 'theme-buttons'));
  assert.equal(group.tag, 'div');
  assert.equal(group.attributes.role, 'group');
  assert.match(group.attributes['aria-label'] ?? '', chinese ? /外观|主题/ : /appearance|theme/i, source.path + ': localized group label');
  assert.equal(Object.hasOwn(group.attributes, 'tabindex'), false, 'Only native buttons enter the tab order');
  assert.equal(source.tags.some(tag => tag.tag === 'select'), false, source.path + ': no theme dropdown remains');
  assert.doesNotMatch(source.html, /data-theme-select\b/i);
  const within = (parent, child) => child.start > parent.start && child.start <= parent.start + source.inner(parent).length;
  const themeTags = source.tags.filter(tag => Object.hasOwn(tag.attributes, 'data-theme-value') || source.hasClass(tag, 'theme-button'));
  assert.equal(themeTags.length, 3, source.path + ': three direct theme choices');
  assert.deepEqual(themeTags.map(tag => tag.attributes['data-theme-value']), preferences);
  assert.deepEqual(source.tags.filter(tag => tag.tag === 'button' && within(group, tag)), themeTags, 'The group contains exactly its three theme buttons');
  for (const name of ['aria-label', 'title']) assert.equal(new Set(themeTags.map(tag => tag.attributes[name])).size, 3, 'Each preference has a distinct accessible label and tooltip');
  const localized = chinese ? { dark: /深色/, light: /浅色/, system: /系统/ } : { dark: /\bdark\b/i, light: /\blight\b/i, system: /\bsystem\b/i };
  for (const control of themeTags) {
    assert.equal(control.tag, 'button');
    assert.ok(source.hasClass(control, 'theme-button'));
    assert.ok(within(group, control), source.path + ': theme button belongs to its labeled group');
    assert.equal(control.attributes.type, 'button');
    for (const name of ['role', 'tabindex', 'aria-checked', 'disabled']) assert.equal(Object.hasOwn(control.attributes, name), false, 'Theme controls retain native button semantics');
    for (const name of ['aria-label', 'title']) assert.match(control.attributes[name] ?? '', localized[control.attributes['data-theme-value']], source.path + ': localized ' + name);
    assert.equal(control.attributes['aria-pressed'], String(control.attributes['data-theme-value'] === 'dark'));
    const icons = source.tags.filter(tag => tag.tag === 'svg' && within(control, tag));
    assert.equal(icons.length, 1, source.path + ': each theme button has one SVG icon');
    assert.equal(icons[0].attributes['aria-hidden'], 'true');
    assert.equal(icons[0].attributes.focusable, 'false');
    assert.match(source.inner(icons[0]), /<(?:path|circle|rect|line|polyline|polygon|ellipse)\b/, 'Theme icons contain real vector artwork');
  }
  assert.equal(themeTags.filter(tag => tag.attributes['aria-pressed'] === 'true').length, 1, 'Dark is the sole no-JS selection');
  for (const tag of [group, ...source.tags.filter(tag => within(group, tag))]) assert.equal(Object.keys(tag.attributes).some(name => /^on/i.test(name)), false, 'No inline control event hooks');
  const page = createPage(source, { stored: 'light' });
  assertTheme(page, 'light'); // Before DOMContentLoaded: this is the first-paint assertion.
  assert.equal(page.root.lang, htmlTag.attributes.lang, 'Theme must never change language');
  assert.deepEqual(page.reads, [key]);
  assert.deepEqual(page.writes, [], 'Merely loading a page never stores anything');
  page.ready();
  assertTheme(page, 'light', 'light');
  const controls = [...page.themeButtons];
  const hrefs = page.downloads.map(link => link.href);
  for (const control of controls) {
    assert.deepEqual(control.listenerTypes(), ['click'], 'Native buttons need only click handlers');
    for (const type of ['keydown', 'keyup']) for (const key of ['Tab', 'Enter', ' ', 'ArrowLeft', 'ArrowRight', 'Home', 'End']) {
      assert.equal(control.dispatch(type, { key }).defaultPrevented, false, 'Theme buttons leave native keyboard behavior intact');
    }
  }
  assert.equal(page.document.listenerTypes().some(type => /^key/.test(type)), false, 'Theme does not install custom keyboard navigation');
  assert.deepEqual(page.writes, [], 'Keyboard events have no synthetic storage side effects');
  page.activeElement = controls.find(button => button.dataset.themeValue === 'dark');
  page.setTheme('dark');
  assertTheme(page, 'dark', 'dark');
  assert.equal(page.activeElement, controls[0]);
  assert.equal(page.focusCalls, 0, 'Theme updates do not steal focus');
  assert.equal(page.themeButtons.length, controls.length);
  for (const [index, control] of controls.entries()) assert.equal(page.themeButtons[index], control, 'Theme button DOM identity is stable');
  assert.deepEqual(page.downloads.map(link => link.href), hrefs, 'Theme changes preserve native downloads');
  assert.deepEqual(page.writes, [[key, 'dark']], 'A click writes only the one validated preference');
  if (!page.diagram) {
    assert.equal(page.root.dataset.motion, 'paused');
    assert.equal(page.observers.length, 0, 'Docs do not install unnecessary diagram observers');
    assert.equal(page.downloads.length, 0, 'Docs/404 have no download elements');
    page.runSite();
    assert.equal(page.menuButton.getAttribute('aria-label'), chinese ? '打开导航' : 'Open navigation');
    page.menuButton.dispatch('click');
    assert.equal(page.menuButton.getAttribute('aria-expanded'), 'true', 'Menu runs without a download section');
    assert.equal(page.menuButton.getAttribute('aria-label'), chinese ? '关闭导航' : 'Close navigation');
    page.document.dispatch('keydown', { key: 'Escape' });
    assert.equal(page.menuButton.getAttribute('aria-expanded'), 'false');
    assert.equal(page.menuButton.getAttribute('aria-label'), chinese ? '打开导航' : 'Open navigation');
    assert.equal(page.activeElement, page.menuButton);
    page.safe();
    const alreadyReady = createPage(source, { readyState: 'complete', stored: 'system', systemDark: false });
    assertTheme(alreadyReady, 'light', 'system');
    assert.equal(alreadyReady.root.dataset.motion, 'paused');
    assert.equal(alreadyReady.observers.length, 0);
    assert.deepEqual(alreadyReady.writes, []);
  }
}
console.log(`PASS: ${pages.length} real HTML pages — synchronous first-paint theme, three localized SVG buttons, native keyboard semantics, localized menu on docs/404, focus/DOM/download preservation and zero extra requests.`);

for (const source of pages.filter(page => page.path === 'index.html' || page.path === 'zh/index.html')) {
  for (const stored of [null, '', 'invalid', 'LIGHT', ' system ', '__proto__', 'zh-CN', 'dark/light']) {
    const page = createPage(source, { stored, systemDark: false });
    assertTheme(page, 'dark');
    page.ready();
    assertTheme(page, 'dark', 'dark');
    assert.deepEqual(page.writes, [], 'Invalid preferences are ignored, not rewritten');
  }
  for (const stored of ['dark', 'light', 'system']) for (const systemDark of [false, true]) {
    const page = createPage(source, { stored, systemDark });
    const effective = stored === 'system' ? systemDark ? 'dark' : 'light' : stored;
    assertTheme(page, effective);
    page.ready();
    assertTheme(page, effective, stored);
    assert.deepEqual(page.writes, [], 'A stored preference is applied without being rewritten');
  }
  const page = createPage(source, { duplicateThemeGroup: true });
  assertTheme(page, 'dark');
  page.ready();
  assertTheme(page, 'dark', 'dark');
  assert.equal(page.themeGroups.length, 2);
  assert.equal(page.themeButtons.length, 6);
  const controls = [...page.themeButtons];
  const hrefs = page.downloads.map(link => link.href);
  const focusedControl = page.themeGroups[1].find(button => button.dataset.themeValue === 'light');
  page.activeElement = focusedControl;
  page.setTheme('light', 1);
  assertTheme(page, 'light', 'light');
  page.scheme.change(true);
  assertTheme(page, 'light', 'light');
  page.setTheme('system');
  assertTheme(page, 'dark', 'system');
  page.scheme.change(false);
  assertTheme(page, 'light', 'system');
  page.setTheme('dark');
  page.scheme.change(false);
  assertTheme(page, 'dark', 'dark');
  const writeCount = page.writes.length;
  const invalidControl = page.themeGroups[1].find(button => button.dataset.themeValue === 'light');
  const originalValue = invalidControl.dataset.themeValue;
  for (const invalid of ['', 'invalid', 'LIGHT', ' system ', '__proto__', 'zh-CN', 'dark/light']) {
    try {
      invalidControl.dataset.themeValue = invalid;
      assert.equal(invalidControl.dispatch('click').defaultPrevented, false);
    } finally {
      invalidControl.dataset.themeValue = originalValue;
    }
    assertTheme(page, 'dark', 'dark');
    assert.equal(page.writes.length, writeCount, 'Invalid control values are ignored, never stored');
  }
  assert.equal(page.activeElement, focusedControl);
  assert.equal(page.focusCalls, 0);
  for (const [index, control] of controls.entries()) assert.equal(page.themeButtons[index], control, 'Synchronized controls keep their DOM identity');
  assert.deepEqual(page.downloads.map(link => link.href), hrefs);
  assert.deepEqual(page.reads, [key]);
  assert.deepEqual(page.writes, ['light', 'system', 'dark'].map(value => [key, value]), 'One write per valid click, none for media changes or invalid data');
  for (const blocked of [{ throwStorageProperty: true }, { throwGet: true }, { throwSet: true }]) {
    const blockedPage = createPage(source, blocked);
    assertTheme(blockedPage, 'dark');
    blockedPage.ready();
    blockedPage.setTheme('light');
    assertTheme(blockedPage, 'light', 'light');
    blockedPage.setTheme('system');
    blockedPage.scheme.change(true);
    assertTheme(blockedPage, 'dark', 'system');
    blockedPage.scheme.change(false);
    assertTheme(blockedPage, 'light', 'system');
    assert.deepEqual(blockedPage.reads, blocked.throwStorageProperty ? [] : [key]);
    assert.deepEqual(blockedPage.writes, blocked.throwStorageProperty ? [] : [[key, 'light'], [key, 'system']], 'Blocked storage never introduces retries or extra operations');
    assert.equal(blockedPage.storage.value, blocked.throwStorageProperty || blocked.throwSet ? null : 'system');
  }
  const withoutMedia = createPage(source, { noMatchMedia: true, noObserver: true });
  assertTheme(withoutMedia, 'dark');
  withoutMedia.ready();
  assertTheme(withoutMedia, 'dark', 'dark');
  withoutMedia.setTheme('light');
  assertTheme(withoutMedia, 'light', 'light');
  withoutMedia.setTheme('system');
  assertTheme(withoutMedia, 'light', 'system');
  withoutMedia.setTheme('dark');
  assertTheme(withoutMedia, 'dark', 'dark');
  assert.deepEqual(withoutMedia.writes, ['light', 'system', 'dark'].map(value => [key, value]));
  const alreadyReady = createPage(source, { readyState: 'complete', stored: 'system', systemDark: true });
  assertTheme(alreadyReady, 'dark', 'system');
  assert.equal(alreadyReady.root.dataset.motion, 'running');
  assert.deepEqual(alreadyReady.writes, []);
}
// Persistence works across different static pages/locales but never saves either
// a language choice or application/user data.
const storage = { value: null };
const firstPage = createPage(pages[0], { storage });
firstPage.ready();
firstPage.setTheme('light');
const nextPage = createPage(pages.find(page => page.path === 'zh/docs/privacy.html'), { storage });
assertTheme(nextPage, 'light');
nextPage.ready();
assertTheme(nextPage, 'light', 'light');
assert.equal(nextPage.root.lang, 'zh-CN');
assert.deepEqual(firstPage.writes, [[key, 'light']]);
assert.deepEqual(nextPage.writes, [], 'Cross-page persistence does not rewrite the preference or locale');
console.log('PASS: dark/light/system, invalid stored/control values, blocked property/getter/setter, system changes, synchronized button groups and exactly one permitted persistent key.');

for (const source of pages.filter(page => page.path === 'index.html' || page.path === 'zh/index.html')) {
  const chinese = source.one(tag => tag.tag === 'html').attributes.lang === 'zh-CN';
  const labels = chinese ? ['暂停动效', '继续动效', '已减少动态效果'] : ['Pause animation', 'Resume animation', 'Reduced motion'];
  const motionTag = source.one(tag => tag.tag === 'button' && Object.hasOwn(tag.attributes, 'data-motion-toggle'));
  assert.equal(motionTag.attributes['aria-pressed'], 'false');
  assert.equal(source.inner(motionTag).trim(), labels[0]);
  const figure = source.one(tag => tag.tag === 'figure' && Object.hasOwn(tag.attributes, 'data-routing-diagram'));
  const diagramHTML = source.inner(figure);
  assert.match(diagramHTML, /<svg\b[^>]*aria-hidden="true"/);
  assert.ok([...diagramHTML.matchAll(/<path\b[^>]*class="[^"]*\broute-flow\b[^"\n]*"/g)].length >= 3, 'Real SVG paths carry the routing flow');
  assert.match(diagramHTML, chinese ? /非实时/ : /not live traffic/);
  assert.match(diagramHTML, chinese ? /默认关闭/ : /Off by default/);
  const page = createPage(source);
  page.ready();
  const assertMotion = (state, pressed, label, disabled = false) => {
    assert.equal(page.root.dataset.motion, state);
    assert.equal(page.button.getAttribute('aria-pressed'), String(pressed));
    assert.equal(page.button.textContent, label);
    assert.equal(page.button.disabled, disabled);
    page.safe();
  };
  assertMotion('running', false, labels[0]); // No observer callback yet: inView defaults true.
  assert.equal(page.observers.length, 1);
  const observer = page.observers[0];
  assert.deepEqual(observer.targets, [page.diagram]);
  observer.emit(false);
  assertMotion('paused', false, labels[0]);
  page.hide(true);
  observer.emit(true);
  assertMotion('paused', false, labels[0]);
  page.hide(false);
  assertMotion('running', false, labels[0]);
  page.button.dispatch('click');
  assertMotion('paused', true, labels[1]);
  observer.emit(false);
  page.hide(true);
  observer.emit(true);
  page.hide(false);
  assertMotion('paused', true, labels[1], false); // Automatic resumes never undo a manual pause.
  observer.emit(false);
  page.button.dispatch('click');
  assertMotion('paused', false, labels[0]); // A resume choice cannot override offscreen pausing.
  observer.emit(true);
  assertMotion('running', false, labels[0]);
  page.reduced.change(true);
  assertMotion('paused', true, labels[2], true);
  page.button.dispatch('click');
  page.hide(true);
  page.hide(false);
  observer.emit(false);
  observer.emit(true);
  assertMotion('paused', true, labels[2], true);
  page.reduced.change(false);
  assertMotion('running', false, labels[0]);
  page.button.dispatch('click');
  page.reduced.change(true);
  assertMotion('paused', true, labels[2], true);
  page.reduced.change(false);
  assertMotion('paused', true, labels[1]);
  page.button.dispatch('click');
  assertMotion('running', false, labels[0]);
  page.setTheme('light');
  assertMotion('running', false, labels[0]);
  assert.equal(page.focusCalls, 0);

  for (const hidden of [false, true]) {
    const reducedPage = createPage(source, { reducedMotion: true, hidden });
    reducedPage.ready();
    assert.equal(reducedPage.root.dataset.motion, 'paused');
    assert.equal(reducedPage.button.textContent, labels[2]);
    assert.equal(reducedPage.button.disabled, true);
    reducedPage.reduced.change(false);
    assert.equal(reducedPage.root.dataset.motion, hidden ? 'paused' : 'running');
    reducedPage.hide(false);
    assert.equal(reducedPage.root.dataset.motion, 'running');
    reducedPage.safe();
  }
  const noObserver = createPage(source, { noObserver: true });
  noObserver.ready();
  assert.equal(noObserver.root.dataset.motion, 'running');
  noObserver.button.dispatch('click');
  assert.equal(noObserver.root.dataset.motion, 'paused');
  const noButton = createPage(source, { noMotionButton: true });
  noButton.ready();
  noButton.reduced.change(true);
  assert.equal(noButton.root.dataset.motion, 'paused');
  noButton.safe();
}
console.log('PASS: bilingual motion controls, initial visibility, manual/observer/background/reduced-motion interplay, unsupported observer and no diagram/button safety.');

const css = await readFile(new URL('../public/styles.css', import.meta.url), 'utf8');
// Extract complete nested at-rule blocks rather than relying on newline layout.
function blocks(pattern, text) {
  return [...text.matchAll(pattern)].map(match => {
    let depth = 1;
    let end = match.index + match[0].length;
    const start = end;
    while (depth && end < text.length) { if (text[end] === '{') depth++; else if (text[end] === '}') depth--; end++; }
    assert.equal(depth, 0, 'CSS block must close');
    return text.slice(start, end - 1);
  });
}
assert.match(css, /\.route-flow\b[^{}]*\{[^{}]*animation\s*:\s*route-flow 3\.5s linear infinite/, 'The approved slightly faster period is 3.5 seconds');
assert.match(css, /\.route-flow--upper\s*\{\s*animation-delay:\s*-1\.05s/, 'Upper branch keeps a 30% phase offset');
assert.match(css, /\.route-flow--middle\s*\{\s*animation-delay:\s*-2\.1s/, 'Middle branch keeps a 60% phase offset');
assert.ok(blocks(/@keyframes\s+[\w-]+\s*\{/g, css).some(block => /stroke-dashoffset\s*:/.test(block)), 'Flow keyframes actually move the SVG stroke');
assert.match(css, /html\s*\[data-motion\s*=\s*["']?paused["']?\][^{]*\.route-flow[^{]*\{[^{}]*animation-play-state\s*:\s*paused/);
const reducedBlocks = blocks(/@media\s*\([^{}]*prefers-reduced-motion\s*:\s*reduce\s*\)[^{]*\{/g, css);
assert.ok(reducedBlocks.length > 0, 'Reduced motion must work without JavaScript too');
assert.ok(reducedBlocks.some(block => /(?:\.route-flow|\*)[^{}]*\{[^{}]*animation\s*:\s*none/.test(block)), 'Reduced-motion CSS disables the routing animation');
assert.doesNotMatch(themeScript, /\bfetch\s*\(|XMLHttpRequest|WebSocket|EventSource|__TAURI__|sessionStorage|document\.cookie/);
assert.equal([...themeScript.matchAll(/\.getItem\s*\(/g)].length, 1);
assert.equal([...themeScript.matchAll(/\.setItem\s*\(/g)].length, 1);
assert.doesNotMatch(themeScript, /\.removeItem\s*\(|\.clear\s*\(/);
assert.doesNotMatch(siteScript, /localStorage|sessionStorage|document\.cookie|__TAURI__|127\.0\.0\.1|localhost/);
assert.equal([...siteScript.matchAll(/\bfetch\s*\(/g)].length, 2, 'No analytics, theme, locale or proxy requests');
console.log('PASS: real SVG/CSS flow, CSS-only reduced motion, pause integration, no forbidden storage/network/desktop APIs.');
