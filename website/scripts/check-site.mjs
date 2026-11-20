import assert from 'node:assert/strict';
import { DOCS_COMMENTS_API, DOCS_COMMENTS_THREAD } from '../src/docs-comments.ts';
import { readdir, readFile, stat } from 'node:fs/promises';
import { join, resolve, sep } from 'node:path';
import { contentRoutes, equivalents, locales, origin, outputPath as root, pathFor, readUI } from './site-locales.mjs';

const expectedRoutes = [...contentRoutes.keys(), '/404'].sort();
const strings = new Map(await Promise.all(locales.map(async locale => [locale.id, await readUI(locale.id)])));
const legacy = new Set(['en', 'zh-Hans']);
async function walk(path) {
  const entries = await readdir(path, { withFileTypes: true });
  return (await Promise.all(entries.map(entry => entry.isDirectory() ? walk(join(path, entry.name)) : join(path, entry.name)))).flat();
}
function attrs(source) {
  return Object.fromEntries([...source.matchAll(/([\w:-]+)\s*=\s*"([^"]*)"/g)].map(([, key, value]) => [key, value]));
}
function tags(html, tag) {
  return [...html.matchAll(new RegExp(`<${tag}\\b([^>]*)>`, 'gi'))].map(match => attrs(match[1]));
}
const htmlAttr = value => value.replace(/[&<>"']/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[char]);
function meta(html, key, value) { return tags(html, 'meta').find(tag => tag[key] === value)?.content; }
const routeOf = path => '/' + path.slice(root.length).replaceAll('\\', '/').replace(/index\.html$/, '').replace(/\.html$/, '');
const files = await walk(root);
const htmls = new Map();
const titles = new Set();
const descriptions = new Set();
let checkedLinks = 0;
let documentedPages = 0;
function checkSchema(value) {
  if (!value || typeof value !== 'object') return;
  assert.ok(!Object.hasOwn(value, 'aggregateRating') && !Object.hasOwn(value, 'review'), 'Do not fabricate ratings/reviews');
  assert.ok(!Object.hasOwn(value, 'alternateName'), 'Do not advertise former brand aliases');
  for (const child of Object.values(value)) if (typeof child === 'object') checkSchema(child);
}
for (const path of files.filter(path => path.endsWith('.html'))) {
  const html = await readFile(path, 'utf8');
  const route = routeOf(path);
  htmls.set(route, html);
  const entry = contentRoutes.get(route);
  assert.ok(entry || route === '/404', 'Unexpected file: ' + route);
  const lang = entry?.locale.lang ?? 'en';
  assert.equal(tags(html, 'html')[0]?.lang, lang, `${route}: language follows the explicit URL, not browser detection`);
  assert.equal([...html.matchAll(/<h1(?:\s|>)/g)].length, 1, `${route}: exactly one H1`);
  const title = html.match(/<title>([^<]+)<\/title>/)?.[1];
  assert.ok(title?.includes('Serylane') && !titles.has(title), `${route}: unique branded title`);
  titles.add(title);
  const description = meta(html, 'name', 'description');
  assert.ok(description?.length >= 20 && !descriptions.has(description), `${route}: unique substantial description`);
  descriptions.add(description);
  assert.ok(meta(html, 'name', 'viewport')?.includes('width=device-width'), `${route}: responsive viewport`);
  assert.doesNotMatch(html, /香港|\bHK\b|Hong Kong/i, `${route}: neutral download-center naming`);
  const brandedContent = html.replace(/https?:\/\/[^\s"'<>]+/g, '');
  assert.doesNotMatch(brandedContent, /RouteDeck|mihomo-codex/i, `${route}: current display brand only`);
  assert.doesNotMatch(html, /<iframe\b|<form\b|<style\b|\sstyle\s*=|\son\w+\s*=/i, `${route}: no inline styles, handlers or embedded services`);
  assert.doesNotMatch(html, /GitHub 备用|Gitee 备用|channel=|https:\/\/github\.com\/CMMUU\/serylane\/releases(?:\/|")|https:\/\/gitee\.com\/[^\s"]*\/releases/i, `${route}: source links are not alternate downloads`);
  const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]);
  assert.equal(ids.length, new Set(ids).size, `${route}: unique IDs`);
  assert.ok(ids.includes('main') && html.includes('href="#main"'), `${route}: working skip target`);
  for (const script of tags(html, 'script')) {
    if (script.src) assert.ok(script.src.startsWith('/') && !script.src.startsWith('//'), `${route}: same-origin scripts only`);
    else assert.equal(script.type, 'application/ld+json', `${route}: only structured data may be inline`);
  }
  assert.ok(tags(html, 'script').some(tag => tag.src === '/theme.js'), `${route}: same-origin pre-paint theme script`);
  if (route === '/404') {
    assert.match(meta(html, 'name', 'robots') ?? '', /noindex/, '404 must not be indexed');
    continue;
  }
  const { page, locale } = entry;
  const rootAttrs = tags(html, 'html')[0];
  assert.equal(rootAttrs.dir, locale.direction);
  assert.equal(rootAttrs['data-site-locale'], locale.id);
  assert.equal(rootAttrs['data-nav-open'], htmlAttr(strings.get(locale.id).open));
  assert.equal(rootAttrs['data-release-loading'], htmlAttr(strings.get(locale.id).loading));
  assert.equal(rootAttrs['data-release-started'], htmlAttr(strings.get(locale.id).started));
  const canonical = tags(html, 'link').filter(tag => tag.rel === 'canonical');
  assert.deepEqual(canonical.map(tag => tag.href), [origin + route], `${route}: exact self canonical`);
  const alternate = tags(html, 'link').filter(tag => tag.rel === 'alternate');
  assert.equal(alternate.length, locales.length + 1, `${route}: no ambiguous alternates`);
  for (const [language, target] of equivalents(page)) {
    assert.equal(alternate.find(tag => tag.hreflang === language)?.href, origin + target, `${route}: reciprocal ${language}`);
  }
  assert.equal(meta(html, 'property', 'og:url'), origin + route);
  assert.equal(meta(html, 'property', 'og:locale'), locale.social);
  assert.equal(meta(html, 'property', 'og:title'), title);
  assert.equal(meta(html, 'property', 'og:description'), description);
  assert.equal(meta(html, 'property', 'og:site_name'), 'Serylane');
  assert.equal(meta(html, 'property', 'og:image'), origin + '/assets/serylane-mark.png');
  assert.equal(meta(html, 'name', 'twitter:card'), 'summary');
  assert.equal(meta(html, 'name', 'twitter:title'), title);
  assert.equal(meta(html, 'name', 'twitter:description'), description);
  assert.equal(meta(html, 'name', 'twitter:image'), origin + '/assets/serylane-mark.png');
  const schemas = [...html.matchAll(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/g)].map(match => JSON.parse(match[1]));
  assert.ok(schemas.length > 0, `${route}: static JSON-LD`);
  for (const schema of schemas) checkSchema(schema);
  const nodes = schemas.flatMap(schema => schema['@graph'] ?? [schema]);
  const webpage = nodes.find(node => node['@type'] === 'WebPage');
  assert.ok(webpage, `${route}: WebPage structured data`);
  assert.equal(webpage.url, origin + route);
  assert.equal(webpage.inLanguage, lang);
  assert.ok(webpage.name === title || htmlAttr(webpage.name) === title, route + ': structured page title');
  if (page === 'home') {
    const app = nodes.find(node => node['@type'] === 'SoftwareApplication');
    assert.equal(app?.name, 'Serylane');
    assert.equal(app.license, 'https://github.com/CMMUU/serylane/blob/main/LICENSE');
    assert.equal(app.operatingSystem, 'Windows, macOS, Linux');
    assert.ok(!app.softwareVersion && !app.offers, 'Do not pin a source release or invent commercial offers');
    const expectedTargets = ['windows-x64','windows-arm64','macos-x64','macos-arm64','linux-x64','linux-arm64'];
    const downloads = tags(html, 'a').filter(link => link['data-download-target']);
    assert.deepEqual(downloads.map(link => link['data-download-target']).sort(), expectedTargets.sort(), route + ': six payload-neutral download links');
    for (const link of downloads) assert.equal(link.href, '/download/' + link['data-download-target']);
  }
  const picker = [...html.matchAll(/<details class="language-menu"[^>]*>[\s\S]*?<\/details>/g)];
  assert.equal(picker.length, 1, route + ': one native language dropdown');
  const navLinks = tags(picker[0][0], 'a');
  assert.deepEqual(navLinks.map(link => link.lang), locales.map(item => item.lang), route + ': menu shows all 11 languages in order');
  for (let i = 0; i < navLinks.length; i++) {
    const link = navLinks[i];
    assert.equal(link.hreflang, locales[i].lang);
    assert.equal(link.href, pathFor(page, locales[i]));
    assert.equal(link['aria-current'], locale.id === locales[i].id ? 'page' : undefined);
  }
  const nav = html.match(/<nav id="main-nav"[^>]*>[\s\S]*?<\/nav>/)[0];
  const mobile = html.match(/<details class="mobile-site-nav"[^>]*>[\s\S]*?<\/details>/)?.[0];
  assert.ok(mobile && tags(mobile, 'summary').length === 1, route + ': native mobile navigation');
  assert.deepEqual(tags(mobile, 'a').map(link => link.href), tags(nav, 'a').map(link => link.href), route + ': same links in mobile and desktop');
  for (const kind of ['guide', 'docs']) assert.ok(tags(nav, 'a').some(link => link.href === pathFor(kind, locale)), route + ': localized entry ' + kind);
  const isDoc = page !== 'home';
  assert.equal(tags(html, 'script').filter(tag => tag.src === '/docs-comments.js').length, Number(isDoc));
  assert.equal(html.includes('class="docs-feedback-layout"'), isDoc);
  assert.equal(html.includes('data-docs-comments='), isDoc);
  if (isDoc) {
    documentedPages++;
    const tocEntries = [...html.matchAll(/<nav class="chapter-nav"[^>]*>[\s\S]*?<\/nav>/g)];
    assert.equal(tocEntries.length, 1, route + ': a single documentation TOC');
    const toc = tocEntries[0][0];
    assert.match(toc, /<details class="chapter-details" open><summary>/, route + ': native collapsible TOC');
    const chapterLinks = tags(toc, 'a').map(link => link.href);
    assert.ok(chapterLinks.length >= 3 && new Set(chapterLinks).size === chapterLinks.length);
    for (const href of chapterLinks) assert.ok(href.startsWith('#') && ids.includes(href.slice(1)), route + ': existing local chapter ' + href);
    assert.ok(html.indexOf(toc) < html.indexOf('class="chapter-content"') && html.indexOf('class="chapter-content"') < html.indexOf('data-docs-comments='));
    assert.equal((html.match(/data-docs-comments="/g) ?? []).length, 1, route + ': one feedback panel');
    assert.ok(html.includes('data-docs-comments="' + DOCS_COMMENTS_API + '"'));
    assert.ok(tags(html, 'a').some(link => link.href === DOCS_COMMENTS_THREAD && link.rel === 'noopener noreferrer'));
  }
  if (page === 'guide') {
    const toc = html.match(/<nav class="chapter-nav"[^>]*>[\s\S]*?<\/nav>/)?.[0];
    assert.ok(toc, route + ': guide table of contents');
    const headings = ['install','quick-start','proxy-modes','subscriptions','nodes','ai-tools','updates','troubleshooting'];
    assert.deepEqual(tags(toc, 'a').map(link => link.href), headings.map(id => '#' + id), route + ': eight stable chapter links');
    assert.ok(headings.every(id => ids.includes(id)), route + ': every chapter link has a target');
    assert.ok(html.includes('<code dir="ltr">codex --enable respect_system_proxy</code>'));
  }
  if (legacy.has(locale.id) && page !== 'guide') {
    const oldMarkup = await readFile(new URL('../public/' + entry.file, import.meta.url), 'utf8');
    for (const [, oldId] of oldMarkup.matchAll(/id="([^"]+)"/g)) assert.ok(ids.includes(oldId), route + ': preserve old anchor ' + oldId);
  }
}
assert.deepEqual([...htmls.keys()].sort(), expectedRoutes, 'Six canonical pages per locale and one genuine 404');
for (const [route, html] of htmls) {
  const entry = contentRoutes.get(route);
  for (const link of tags(html, 'a')) {
    if (!entry || !link.href || Object.hasOwn(link, 'data-language-link')) continue;
    const destination = new URL(link.href, origin + route);
    const targetEntry = destination.origin === origin && contentRoutes.get(destination.pathname);
    if (targetEntry && targetEntry.locale.id !== entry.locale.id) {
      assert.equal(link.hreflang, targetEntry.locale.lang, route + ': unlabelled cross-language link ' + link.href);
      assert.ok(destination.hash, route + ': only explicitly linked archive details may switch language');
    }
  }
  for (const match of html.matchAll(/\b(?:href|src)="([^"]+)"/g)) {
    const href = match[1].replaceAll('&amp;', '&');
    assert.ok(!href.startsWith('//'), `${route}: no external protocol-relative assets`);
    const target = new URL(href, origin + route);
    if (target.origin !== origin) continue;
    if (/^\/download\/(windows|macos|linux)-(x64|arm64)$/.test(target.pathname)) {
      assert.equal(target.search, '', 'Download URLs must not receive language/theme/channel overrides');
      checkedLinks++; continue;
    }
    const plainRoute = target.pathname.replace(/index\.html$/, '').replace(/\.html$/, '');
    const targetHtml = htmls.get(plainRoute);
    if (!targetHtml) {
      const path = resolve(root, '.' + target.pathname);
      assert.ok(path.startsWith(resolve(root) + sep));
      assert.ok((await stat(path).catch(() => null))?.isFile(), `${route}: missing local asset ${href}`);
    }
    if (target.hash && targetHtml) assert.ok(targetHtml.includes(`id="${decodeURIComponent(target.hash.slice(1))}"`), `${route}: broken anchor ${href}`);
    checkedLinks++;
  }
}
const legacyOverviewAnchors = ['openai-ranking','subscription-cards','progressive-diagnostics','proxy-routing-state','tun-readiness','start','download-center','subscription-pipeline','update-routing','windows-session-exit','interaction-response','ai-health','node-testing','subscription-candidate','tun-compatibility','windows-tun','compact-menu-bar','app-binding','fast-proxy-start','quiet-startup'];
for (const prefix of ['', '/zh']) {
  for (const id of legacyOverviewAnchors) assert.ok(htmls.get(prefix + '/docs/').includes(`id="${id}"`), `${prefix}: preserve historical doc anchor ${id}`);
  const home = htmls.get(prefix + '/');
  assert.ok(home.includes('id="features"') && home.includes('id="capabilities"'), 'Preserve the old homepage feature anchor');
  assert.match(home, /off by default|默认关闭/i, 'Routing is optional, never silently enabled');
  assert.match(home, /not live traffic|非实时连接/i, 'The routing diagram is illustrative');
  assert.ok(home.includes('https://github.com/CMMUU/serylane'), 'Visible source destination');
  for (const system of ['windows','macos','linux']) for (const architecture of ['x64','arm64']) {
    assert.ok(home.includes(`href="/download/${system}-${architecture}"`), `${prefix}: six no-JS routes`);
  }
}
assert.equal(documentedPages, 55, 'All five kinds of documentation in eleven locales');
const sitemap = await readFile(join(root, 'sitemap.xml'), 'utf8');
const listed = [...sitemap.matchAll(/<loc>([^<]+)<\/loc>/g)].map(match => match[1]).sort();
assert.deepEqual(listed, expectedRoutes.filter(route => route !== '/404').map(route => origin + route).sort(), 'Sitemap has all canonical content pages, no error/API/download routes');
assert.match(sitemap, /xmlns:xhtml="http:\/\/www\.w3\.org\/1999\/xhtml"/);
for (const block of sitemap.matchAll(/<url>([\s\S]*?)<\/url>/g)) {
  const route = block[1].match(/<loc>([^<]+)<\/loc>/)[1].slice(origin.length);
  for (const [lang, path] of equivalents(contentRoutes.get(route).page)) {
    assert.ok(block[1].includes(`hreflang="${lang}" href="${origin}${path}"`), 'Sitemap alternates are reciprocal');
  }
}
const robots = await readFile(join(root, 'robots.txt'), 'utf8');
assert.match(robots, /Sitemap: https:\/\/serylane\.cmmuu\.com\/sitemap\.xml/);
assert.match(robots, /Allow: \//);
const script = await readFile(join(root, 'site.js'), 'utf8');
assert.doesNotMatch(script.replace(/["']hk["']/g, 'internalChannel'), /香港|\bHK\b|Hong Kong/i);
assert.equal([...script.matchAll(/\bfetch\s*\(/g)].length, 2, 'Only the two existing public download API operations');
assert.match(script, /fetch\(\s*["']\/api\/releases\/latest["']/);
assert.match(script, /fetch\(\s*`\/api\/download\/\$\{target\}/);
assert.doesNotMatch(script, /version:\s*["']v\d+\./, 'No pinned release snapshot');
for (const path of files.filter(path => path.endsWith('.js'))) {
  const source = await readFile(path, 'utf8');
  assert.doesNotMatch(source, /\b(?:XMLHttpRequest|WebSocket|invoke|EventSource)\s*\(|127\.0\.0\.1|localhost|__TAURI__|sessionStorage|document\.cookie|sendBeacon/, `${path}: no local proxy, desktop, tracking or arbitrary storage APIs`);
  if (path === join(root, 'theme.js')) {
    assert.equal([...source.matchAll(/window\.localStorage\.getItem\("serylane-theme"\)/g)].length, 1, 'Only one approved preference read');
    assert.equal([...source.matchAll(/window\.localStorage\.setItem\("serylane-theme", preference\)/g)].length, 1, 'Only one approved preference write');
    const narrowed = source.replace(/window\.localStorage\.getItem\("serylane-theme"\)/g, 'approvedThemeRead()').replace(/window\.localStorage\.setItem\("serylane-theme", preference\)/g, 'approvedThemeWrite()');
    assert.doesNotMatch(narrowed, /localStorage|\bfetch\s*\(/, 'Theme persistence is not a blanket storage/network exemption');
  } else assert.doesNotMatch(source, /localStorage/, `${path}: no other script may persist data`);
}
const commentsScript = await readFile(join(root, 'docs-comments.js'), 'utf8');
assert.equal([...commentsScript.matchAll(/\bfetch\s*\(/g)].length, 1);
assert.match(commentsScript, /fetch\(panel\.dataset\.docsComments,\s*\{ credentials: "omit"/);
assert.doesNotMatch(commentsScript, /innerHTML|outerHTML|insertAdjacentHTML/);
const headers = await readFile(join(root, '_headers'), 'utf8');
assert.match(headers, /script-src 'self'; style-src 'self'/);
assert.doesNotMatch(headers, /unsafe-inline|unsafe-eval/);
const config = JSON.parse(await readFile(new URL('../wrangler.jsonc', import.meta.url), 'utf8'));
assert.deepEqual(config.routes, [{ pattern: 'serylane.cmmuu.com', custom_domain: true }]);
assert.equal(config.assets.not_found_handling, '404-page');
assert.equal(config.workers_dev, false);
assert.deepEqual(config.assets.run_worker_first, ['/api/*', '/download/*']);
assert.equal(config.assets.binding, 'ASSETS');
console.log(`PASS: ${htmls.size} static HTML pages, ${checkedLinks} local links/assets; 55 docs with shared TOC and feedback; 11-language dropdown/canonical/hreflang/sitemap/JSON-LD, historical anchors, CSP, one-key theme privacy, and allowlisted download/comments network scope.`);
