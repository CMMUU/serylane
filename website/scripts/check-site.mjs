import assert from 'node:assert/strict';
import { readdir, readFile, stat } from 'node:fs/promises';
import { join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../public/', import.meta.url));
const origin = 'https://serylane.cmmuu.com';
const pairs = ['/', '/docs/', '/docs/routing', '/docs/verification', '/docs/privacy'];
const expectedRoutes = [...pairs, ...pairs.map(route => '/zh' + route), '/404'].sort();
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
function meta(html, key, value) { return tags(html, 'meta').find(tag => tag[key] === value)?.content; }
const routeOf = path => '/' + path.slice(root.length).replaceAll('\\', '/').replace(/index\.html$/, '').replace(/\.html$/, '');
const files = await walk(root);
const htmls = new Map();
const titles = new Set();
const descriptions = new Set();
let checkedLinks = 0;
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
  const lang = route.startsWith('/zh/') ? 'zh-CN' : 'en';
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
  const canonical = tags(html, 'link').filter(tag => tag.rel === 'canonical');
  assert.deepEqual(canonical.map(tag => tag.href), [origin + route], `${route}: exact self canonical`);
  const english = route.startsWith('/zh/') ? route.slice(3) : route;
  const alternate = tags(html, 'link').filter(tag => tag.rel === 'alternate');
  assert.equal(alternate.length, 3, `${route}: no ambiguous alternates`);
  for (const [language, target] of [['en', english], ['zh-CN', '/zh' + english], ['x-default', english]]) {
    assert.equal(alternate.find(tag => tag.hreflang === language)?.href, origin + target, `${route}: reciprocal ${language}`);
  }
  assert.equal(meta(html, 'property', 'og:url'), origin + route);
  assert.equal(meta(html, 'property', 'og:locale'), lang === 'en' ? 'en_US' : 'zh_CN');
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
  assert.equal(webpage.name, title);
  if (pairs.includes(route) && route === '/') {
    const app = nodes.find(node => node['@type'] === 'SoftwareApplication');
    assert.equal(app?.name, 'Serylane');
    assert.equal(app.license, 'https://github.com/CMMUU/serylane/blob/main/LICENSE');
    assert.equal(app.operatingSystem, 'Windows, macOS, Linux');
    assert.ok(!app.softwareVersion && !app.offers, 'Do not pin a source release or invent commercial offers');
  }
}
assert.deepEqual([...htmls.keys()].sort(), expectedRoutes, 'English and Chinese each have a homepage and four complete docs; one 404');
for (const [route, html] of htmls) {
  for (const match of html.matchAll(/\b(?:href|src)="([^"]+)"/g)) {
    const href = match[1].replaceAll('&amp;', '&');
    if (/^(https?:|mailto:)/.test(href)) continue;
    assert.ok(!href.startsWith('//'), `${route}: no external protocol-relative assets`);
    const target = new URL(href, origin + route);
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
const sitemap = await readFile(join(root, 'sitemap.xml'), 'utf8');
const listed = [...sitemap.matchAll(/<loc>([^<]+)<\/loc>/g)].map(match => match[1]).sort();
assert.deepEqual(listed, expectedRoutes.filter(route => route !== '/404').map(route => origin + route).sort(), 'Sitemap has all ten canonical content pages, no error/API/download routes');
assert.match(sitemap, /xmlns:xhtml="http:\/\/www\.w3\.org\/1999\/xhtml"/);
for (const block of sitemap.matchAll(/<url>([\s\S]*?)<\/url>/g)) {
  const route = block[1].match(/<loc>([^<]+)<\/loc>/)[1].slice(origin.length);
  const english = route.startsWith('/zh/') ? route.slice(3) : route;
  for (const [lang, path] of [['en', english], ['zh-CN', '/zh' + english], ['x-default', english]]) {
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
const headers = await readFile(join(root, '_headers'), 'utf8');
assert.match(headers, /script-src 'self'; style-src 'self'/);
assert.doesNotMatch(headers, /unsafe-inline|unsafe-eval/);
const config = JSON.parse(await readFile(new URL('../wrangler.jsonc', import.meta.url), 'utf8'));
assert.deepEqual(config.routes, [{ pattern: 'serylane.cmmuu.com', custom_domain: true }]);
assert.equal(config.assets.not_found_handling, '404-page');
assert.equal(config.workers_dev, false);
assert.deepEqual(config.assets.run_worker_first, ['/api/*', '/download/*']);
assert.equal(config.assets.binding, 'ASSETS');
console.log(`PASS: ${htmls.size} static HTML pages, ${checkedLinks} local links/assets; bilingual canonical/hreflang/sitemap/JSON-LD, historical anchors, CSP, one-key theme privacy, and download-only network scope.`);
