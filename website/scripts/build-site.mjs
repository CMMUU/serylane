import assert from 'node:assert/strict';
import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { contentRoutes, english, equivalents, locales, origin, output, outputPath, pathFor, readUI } from './site-locales.mjs';

const source = new URL('../public/', import.meta.url);
const content = new URL('../content/', import.meta.url);
const publicText = path => readFile(new URL(path, source), 'utf8');
const contentText = path => readFile(new URL(path, content), 'utf8');
const esc = value => String(value).replace(/[&<>"']/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[char]);
const decode = value => value.replace(/&(amp|lt|gt|quot|#39|#x27);/g, (_, entity) =>
  ({ amp: '&', lt: '<', gt: '>', quot: '"', '#39': "'", '#x27': "'" })[entity]);
const git = 'https://github.com/CMMUU/serylane';
const logo = '/assets/serylane-mark.png';
const idsOf = html => new Set([...html.matchAll(/\bid="([^"]+)"/g)].map(([, id]) => id));
const bodyOf = html => html.match(/<body[^>]*>([\s\S]*)<\/body>/)[1];
const json = value => JSON.stringify(value).replaceAll('<', '\\u003c');
const data = (attrs) => Object.entries(attrs).map(([key, value]) => ' data-' + key + '="' + esc(value) + '"').join('');
const icon = path => '<svg class="icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path d="' + path + '"/></svg>';
const globe = '<svg class="icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false"><circle cx="12" cy="12" r="9"/><ellipse cx="12" cy="12" rx="4" ry="9"/><path d="M3 12h18"/></svg>';
const navAttributes = 'aria-controls="main-nav" aria-expanded="false"';

function legacy(html, page, locale, ui) {
  const alternate = headAlternates(page);
  // Only replace the common language UI and SEO links. Historical copy and anchors remain intact.
  html = html.replace(/(?:[ \t]*<link rel="alternate" hreflang="[^"]+" href="[^"]+">\n)+/, alternate + '\n');
  assert.ok(html.includes('hreflang="ar"'), page + ': expected legacy alternate block');
  html = html.replace(/<html([^>]*)>/, (_, attributes) => '<html' + attributes + htmlData(locale, ui) + '>');
  html = html.replace(/"inLanguage":\["en","zh-CN"\]/g, '"inLanguage":' + json(locales.map(item => item.lang)));
  html = html.replace(/<div class="locale-switch"[^>]*>[\s\S]*?<\/div>/, '<!-- LANGUAGE -->');
  html = html.replace(/(<nav id="main-nav"[^>]*>[\s\S]*?)(<\/nav>)/, '$1' + guideLink(locale, ui) + '$2');
  if (page === 'docs') html = html.replace(/(<p class="lead">[\s\S]*?<\/p>)/, '$1<p class="guide-entry">' + guideLink(locale, ui) + '</p>');
  return html;
}

function htmlData(locale, ui) {
  return ' dir="' + locale.direction + '"' + data({
    'site-locale': locale.id, 'nav-open': ui.open, 'nav-close': ui.close, 'motion-pause': ui.pause, 'motion-resume': ui.resume,
    'motion-reduced': ui.reduced, 'release-loading': ui.loading, 'release-latest': ui.latest, 'release-unavailable': ui.unavailable,
    'release-checking': ui.checking, 'release-started': ui.started, 'release-failed': ui.failed, 'release-retry': ui.retry,
  });
}
function headAlternates(page) {
  return equivalents(page).map(([lang, route]) => '<link rel="alternate" hreflang="' + lang + '" href="' + origin + route + '">').join('\n');
}
function guideLink(locale, ui, active = false) {
  return '<a class="guide-link" href="' + pathFor('guide', locale) + '"' + (active ? ' aria-current="page"' : '') + '>' + esc(ui.guide) + '</a>';
}
function picker(page, locale, ui, idsByRoute) {
  const currentIds = idsByRoute.get(pathFor(page, locale));
  const links = locales.map(target => {
    const active = locale.id === target.id;
    const targetPath = pathFor(page, target);
    const anchors = [...idsByRoute.get(targetPath)].filter(id => currentIds.has(id));
    return '<li><a href="' + targetPath + '" lang="' + target.lang + '" hreflang="' + target.lang + '" dir="auto"' +
      (active ? ' aria-current="page"' : '') + data({ 'language-link': '', 'language-anchors': anchors.join(' ') }) + '>' +
      '<bdi>' + esc(target.name) + '</bdi>' + (active ? '<span aria-hidden="true">✓</span>' : '') + '</a></li>';
  });
  return '<details class="language-menu" data-language-menu><summary aria-label="' + esc(ui.language + ': ' + locale.name) + '">' +
    globe + '<bdi class="language-current" lang="' + locale.lang + '">' + esc(locale.name) + '</bdi>' +
    icon('m6 9 6 6 6-6') + '</summary><ul class="language-options" aria-label="' + esc(ui.language) + '">' + links.join('') + '</ul></details>';
}

function nativeMobileNav(html, ui) {
  const nav = html.match(/<nav id="main-nav"[^>]*>[\s\S]*?<\/nav>/)?.[0];
  const toggle = html.match(/<button class="menu-toggle"[^>]*>[\s\S]*?<\/button>/)?.[0];
  assert.ok(nav && toggle, 'Expected the shared desktop nav and mobile trigger');
  const list = nav.replace(' id="main-nav"', '');
  const mobile = '<details class="mobile-site-nav" data-mobile-nav><summary class="menu-toggle" aria-label="' +
    esc(ui.nav) + '">' + icon('M4 6h16M4 12h16M4 18h16') + '</summary>' + list + '</details>';
  return html.replace(toggle, '').replace('</header>', () => mobile + '</header>');
}

function head(page, locale, title, description) {
  const url = origin + pathFor(page, locale);
  const websiteId = origin + '/#website';
  const schema = { '@context': 'https://schema.org', '@graph': [
    { '@type': 'WebSite', '@id': websiteId, url: origin + '/', name: 'Serylane', inLanguage: locales.map(item => item.lang) },
    { '@type': 'WebPage', '@id': url + '#webpage', url, name: title, description, inLanguage: locale.lang, isPartOf: { '@id': websiteId } },
  ] };
  if (page === 'home') schema['@graph'].push({
    '@type': 'SoftwareApplication', '@id': origin + '/#software', name: 'Serylane', url, description,
    applicationCategory: 'UtilitiesApplication', operatingSystem: 'Windows, macOS, Linux', isAccessibleForFree: true,
    license: git + '/blob/main/LICENSE', downloadUrl: url + '#download', softwareHelp: origin + pathFor('guide', locale),
  });
  const metatags = [
    ['og:title', title], ['og:description', description], ['og:site_name', 'Serylane'], ['og:type', 'website'],
    ['og:url', url], ['og:locale', locale.social], ['og:image', origin + logo],
    ['twitter:card', 'summary'], ['twitter:title', title], ['twitter:description', description], ['twitter:image', origin + logo],
  ].map(([name, value]) => '<meta ' + (name.startsWith('og:') ? 'property' : 'name') + '="' + name + '" content="' + esc(value) + '">');
  return [
    '<meta charset="utf-8">', '<meta name="viewport" content="width=device-width, initial-scale=1">',
    '<meta name="theme-color" content="#0a0d14">', '<title>' + esc(title) + '</title>',
    '<meta name="description" content="' + esc(description) + '">', '<link rel="canonical" href="' + url + '">',
    headAlternates(page), ...metatags, '<link rel="icon" type="image/png" href="' + logo + '">',
    '<script src="/theme.js"></script>', '<link rel="stylesheet" href="/styles.css">',
    '<script src="/site.js" defer></script>',
    '<script type="application/ld+json">' + json(schema) + '</script>',
  ].join('\n');
}

const homeStrings = JSON.parse(await contentText('home/source.json'));
const enHomepage = await publicText('index.html');
function translateHome(body, textMap) {
  const used = new Set();
  const translate = value => {
    const key = decode(value.trim());
    if (!key) return value;
    assert.ok(textMap.has(key), 'New or changed home string: ' + key + '; update the translations');
    used.add(key);
    return value.replace(value.trim(), () => esc(textMap.get(key)));
  };
  const translated = body.match(/<[^>]+>|[^<]+/g).map(piece => piece.startsWith('<')
    ? piece.replace(/\b(aria-label|title|alt)="([^"]+)"/g, (_, attr, value) => attr + '="' + translate(value) + '"')
    : translate(piece)).join('');
  assert.deepEqual(used, new Set(textMap.keys()), 'Homepage translation keys match the current English layout');
  return translated;
}

async function homeFor(locale, ui) {
  const raw = (await contentText('home/' + locale.id + '.txt')).trimEnd().split(/\r?\n/);
  const [title, description, ...texts] = raw;
  assert.ok(title && description && texts.every(text => text.trim()), 'Missing home copy: ' + locale.id);
  const required = homeStrings.filter(entry => !entry.ui && !entry.invariant && !entry.package);
  assert.deepEqual(texts.map(text => Number(text.split('\t', 1)[0])), required.map(entry => entry.id), locale.id + ': home keys');
  const extra = Object.fromEntries(texts.map(text => [text.split('\t', 1)[0], text.slice(text.indexOf('\t') + 1)]));
  const map = new Map(homeStrings.map(entry => {
    const text = entry.ui ? ui[entry.ui] : entry.invariant ? entry.source : entry.package
      ? ui.package.replace('{target}', entry.package === 'mac-arm' ? 'macOS ' + extra[67] + ' DMG' : entry.package)
      : extra[entry.id];
    assert.ok(text?.trim(), 'Missing home string: ' + locale.id + ':' + entry.id);
    return [entry.source, text];
  }));
  const bare = bodyOf(enHomepage).replace(/<div class="locale-switch"[^>]*>[\s\S]*?<\/div>/, '<!-- LANGUAGE -->');
  const body = translateHome(bare, map).replace(/\bhref="([^"]+)"/g, (tag, href) => {
    const url = new URL(href, origin);
    const page = url.origin === origin && href.startsWith('/') && contentRoutes.get(url.pathname)?.page;
    return page ? 'href="' + pathFor(page, locale) + url.hash + '"' : tag;
  }).replace(/(<nav id="main-nav"[^>]*>[\s\S]*?)(<\/nav>)/, '$1' + guideLink(locale, ui) + '$2');
  return '<!doctype html>\n<html lang="' + locale.lang + '" data-theme="system" data-motion="running"' + htmlData(locale, ui) +
    '>\n<head>\n' + head('home', locale, title, description) + '\n</head>\n<body>' + body + '</body>\n</html>\n';
}

function docsHeader(locale, page, ui) {
  const theme = enHomepage.match(/<div class="theme-buttons"[^>]*>[\s\S]*?<\/div>/)[0]
    .replace(/(aria-label|title)="([^"]+)"/g, (_, attr, value) => attr + '="' + esc(({ Appearance: ui.appearance, 'Dark mode': ui.dark, 'Light mode': ui.light, 'Follow system': ui.system })[value]) + '"');
  const root = pathFor('home', locale);
  return [
    '<a class="skip-link" href="#main">' + esc(ui.skip) + '</a>',
    '<header class="site-header page-width">',
    '<a class="brand" href="' + root + '" aria-label="' + esc(ui.home) + '"><img src="' + logo + '" width="32" height="32" alt=""><span>Serylane</span></a>',
    '<button class="menu-toggle" type="button" aria-label="' + esc(ui.open) + '" ' + navAttributes + '>' + icon('M4 6h16M4 12h16M4 18h16') + '</button>',
    '<nav id="main-nav" class="main-nav" aria-label="' + esc(ui.nav) + '"><a href="' + root + '#capabilities">' + esc(ui.capabilities) + '</a>',
    '<a href="' + pathFor('docs', locale) + '"' + (page === 'docs' ? ' aria-current="page"' : '') + '>' + esc(ui.docs) + '</a>',
    guideLink(locale, ui, page === 'guide'), '<a href="' + git + '" target="_blank" rel="noopener noreferrer">' + esc(ui.source) + '</a>',
    '<a class="button button-secondary nav-download" href="' + root + '#download">' + esc(ui.download) + '</a></nav>',
    '<div class="header-controls"><!-- LANGUAGE -->' + theme + '</div>',
    '</header>',
  ].join('\n');
}

export function parseArticle(markdown) {
  const lines = markdown.trim().replaceAll('\r', '').replace(/^## ([^\n]+)\n<!-- anchor: ([a-z][a-z0-9-]*) -->$/gm, '## $1 {#$2}').split('\n');
  assert.match(lines[0], /^# .+/);
  const title = lines.shift().slice(2);
  const lead = [];
  const sections = [];
  let into = lead;
  for (const line of lines) {
    const chapter = line.match(/^## (.+) \{#([a-z][a-z0-9-]*)\}$/);
    if (chapter) {
      const [, label, id] = chapter;
      assert.ok(!sections.some(item => item.id === id), 'Duplicate chapter: ' + id);
      into = [];
      sections.push({ id, label, lines: into });
    } else {
      assert.ok(!line.startsWith('#'), 'Use ## Heading {#id} for a chapter: ' + line);
      into.push(line);
    }
  }
  const description = lead.join(' ').trim().replace(/\s{2,}/g, ' ');
  assert.ok(description.length >= 20 && sections.length >= 3, title + ': lead and chapters required');
  for (const section of sections) assert.ok(section.lines.join('').trim(), 'Empty chapter: ' + section.id);
  return { title, description, sections };
}

function renderBlocks(lines, locale) {
  const inline = text => {
    const pattern = /(\[([^\]]+)\]\(([^)\s]+)\)|\x60([^\x60]+)\x60|\*\*([^*]+)\*\*)/g;
    let out = '', start = 0;
    for (const match of text.matchAll(pattern)) {
      out += esc(text.slice(start, match.index));
      const [, token, label, target, code, bold] = match;
      if (code) out += '<code dir="ltr">' + esc(code) + '</code>';
      else if (bold) out += '<strong>' + esc(bold) + '</strong>';
      else {
        let href = target;
        if (target.startsWith('site:')) {
          const [, page, fragment] = target.match(/^site:(home|docs|guide|routing|verification|privacy)(#[a-z0-9-]+)?$/);
          href = pathFor(page, locale) + (fragment ?? '');
        }
        const url = new URL(href, origin);
        assert.equal(url.protocol, 'https:', token);
        assert.ok([origin, 'https://github.com', 'https://downloads.cmmuu.com', 'https://learn.chatgpt.com'].includes(url.origin), 'Unexpected guide link: ' + target);
        if (url.origin === origin) href = url.pathname + url.search + url.hash;
        const destination = url.origin === origin ? contentRoutes.get(url.pathname) : undefined;
        const archive = destination && destination.locale.id !== locale.id;
        if (archive) assert.ok(['#history', '#verification-history'].includes(url.hash), 'Nonlocalized navigation: ' + target);
        const flags = archive ? ' hreflang="' + destination.locale.lang + '" data-legacy-language-link' : url.origin !== origin ? ' rel="noopener noreferrer" target="_blank"' : '';
        out += '<a href="' + esc(href) + '"' + flags + '>' + esc(label) + '</a>';
      }
      start = match.index + match[0].length;
    }
    return out + esc(text.slice(start));
  };
  return lines.join('\n').trim().split(/\n\s*\n/).map(block => {
    const rows = block.split('\n');
    const list = rows.every(row => /^- /.test(row)) ? ['ul', /^- /] : rows.every(row => /^\d+\. /.test(row)) ? ['ol', /^\d+\. /] : null;
    if (!list) return '<p>' + inline(rows.join(' ')) + '</p>';
    return '<' + list[0] + '>' + rows.map(row => '<li>' + inline(row.replace(list[1], '')) + '</li>').join('') + '</' + list[0] + '>';
  }).join('\n');
}

const guideChapters = ['install', 'quick-start', 'proxy-modes', 'subscriptions', 'nodes', 'ai-tools', 'updates', 'troubleshooting'];
const pageChapters = {
  docs: ['start', 'current-capabilities', 'tun-compatibility', 'download-center', 'history'],
  routing: ['node-metadata', 'save-apply', 'global-controls', 'enablement', 'proxy-routing-state', 'stability', 'validation-boundaries'],
  verification: ['download-origin', 'checksums', 'update-signatures', 'system-signing', 'macos-tun', 'update-downloads', 'update-installation', 'verification-history'],
  privacy: ['website-data', 'theme-preference', 'hosting', 'downloads', 'desktop-client', 'reporting'],
};
const related = (page, locale, ui) => '<nav class="document-nav" aria-label="' + esc(ui.related) + '">' +
  ['docs', 'guide', 'routing', 'verification', 'privacy'].filter(key => key !== page).map(key =>
    '<a href="' + pathFor(key, locale) + '">' + esc(ui[key]) + '</a>').join('') + '</nav>';

function articleFor(markdown, page, locale, ui) {
  const { title, description, sections } = parseArticle(markdown);
  assert.deepEqual(sections.map(item => item.id), page === 'guide' ? guideChapters : pageChapters[page], locale.id + ':' + page + ' chapter IDs');
  const toc = '<nav class="chapter-nav" aria-label="' + esc(ui.toc) + '" data-chapter-nav><h2>' + esc(ui.toc) + '</h2><ol>' +
    sections.map(section => '<li><a href="#' + section.id + '">' + esc(section.label) + '</a></li>').join('') + '</ol></nav>';
  const guide = page === 'guide';
  const sectionsHtml = sections.map(({ id, label, lines }) => '<section id="' + id + '" data-chapter><h2>' +
    esc(label) + '</h2>\n' + renderBlocks(lines, locale) + '\n</section>').join('\n');
  return [
    '<!doctype html>', '<html lang="' + locale.lang + '" data-theme="system"' + htmlData(locale, ui) + '>', '<head>',
    head(page, locale, title.includes('Serylane') ? title : title + ' — Serylane', description), '</head>', '<body class="document-page">',
    docsHeader(locale, page, ui), '<main id="main" class="document-main page-width' + (guide ? ' guide-main' : '') + '">',
    '<p class="breadcrumb"><a href="' + pathFor('home', locale) + '">Serylane</a> / ' +
      (page !== 'docs' ? '<a href="' + pathFor('docs', locale) + '">' + esc(ui.docs) + '</a> / ' : '') +
      '<span aria-current="page">' + esc(ui[page]) + '</span></p>',
    '<h1>' + esc(title) + '</h1>', '<p class="lead">' + esc(description) + '</p>',
    (page === 'docs' ? '<p class="guide-entry">' + guideLink(locale, ui) + '</p>' : ''),
    '<div class="' + (guide ? 'guide-layout' : 'article-layout') + '">' + toc +
      '<div class="chapter-content">' + sectionsHtml + '</div></div>',
    related(page, locale, ui), '</main>',
    '<footer class="document-footer"><div class="page-width"><a href="' + pathFor('home', locale) + '">Serylane</a> · ' +
      '<a href="' + pathFor('docs', locale) + '">' + esc(ui.docs) + '</a> · ' +
      '<a href="' + git + '" target="_blank" rel="noopener noreferrer">' + esc(ui.source) + '</a> · ' +
      '<a href="' + git + '/blob/main/LICENSE" target="_blank" rel="noopener noreferrer">GPL-3.0-only</a></div></footer>',
    '</body>', '</html>', '',
  ].join('\n');
}

export async function buildSite() {
  const translations = new Map(await Promise.all(locales.map(async locale => [locale.id, await readUI(locale.id)])));
  const legacyLocales = [english.id, 'zh-Hans'];
  const articles = new Map(await Promise.all(locales.filter(locale => !legacyLocales.includes(locale.id)).map(async locale => {
    const pages = (await contentText('pages/' + locale.id + '.md')).split(/^<!-- (docs|routing|verification|privacy) -->\r?$/m);
    const map = new Map();
    assert.equal(pages.length, 9, locale.id + ': four main articles');
    for (let i = 1; i < pages.length; i += 2) map.set(pages[i], pages[i + 1]);
    return [locale.id, map];
  })));
  const bodies = new Map(await Promise.all([...contentRoutes].map(async ([route, { page, locale, file }]) => {
    const ui = translations.get(locale.id);
    const html = page === 'guide' ? articleFor(await contentText('guide/' + locale.id + '.md'), page, locale, ui)
      : legacyLocales.includes(locale.id) ? legacy(await publicText(file), page, locale, ui)
      : page === 'home' ? await homeFor(locale, ui)
      : articleFor(articles.get(locale.id).get(page), page, locale, ui);
    return [route, html];
  })));
  const idsByRoute = new Map([...bodies].map(([route, html]) => [route, idsOf(html)]));
  await rm(output, { recursive: true, force: true });
  await cp(source, output, { recursive: true, filter: filename => !filename.endsWith('/build-info.json') });
  await Promise.all([...contentRoutes].map(async ([route, { page, locale, file }]) => {
    const html = bodies.get(route);
    assert.equal(html.split('<!-- LANGUAGE -->').length, 2, route + ': one shared language picker');
    const ui = translations.get(locale.id);
    const hydrated = nativeMobileNav(html, ui).replace('<!-- LANGUAGE -->', () => picker(page, locale, ui, idsByRoute))
      .replace('<script src="/site.js" defer></script>', '<script src="/site.js" defer></script>\n<script src="/language.js" defer></script>');
    const target = new URL(file, output);
    await mkdir(dirname(fileURLToPath(target)), { recursive: true });
    await writeFile(target, hydrated);
  }));
  const sitemap = [...contentRoutes].map(([route, { page }]) => '<url><loc>' + origin + route + '</loc>\n' +
    equivalents(page).map(([lang, path]) => '<xhtml:link rel="alternate" hreflang="' + lang + '" href="' + origin + path + '"/>').join('\n') + '\n</url>');
  await writeFile(new URL('sitemap.xml', output), '<?xml version="1.0" encoding="UTF-8"?>\n' +
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">\n' + sitemap.join('\n') + '\n</urlset>\n');
  console.log('Built ' + contentRoutes.size + ' pages across ' + locales.length + ' locales → ' + outputPath);
}
if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) await buildSite();
