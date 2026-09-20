import assert from 'node:assert/strict';
import { readdir, readFile, stat } from 'node:fs/promises';
import { join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../public/', import.meta.url));
const origin = 'https://serylane.cmmuu.com';
async function walk(path) {
  const entries = await readdir(path, { withFileTypes: true });
  return (await Promise.all(entries.map(entry => entry.isDirectory() ? walk(join(path, entry.name)) : join(path, entry.name)))).flat();
}
const files = await walk(root);
const htmls = new Map();
let checkedLinks = 0;
const routeOf = path => '/' + path.slice(root.length).replaceAll('\\', '/').replace(/index\.html$/, '').replace(/\.html$/, '');
for (const path of files.filter(path => path.endsWith('.html'))) {
  const html = await readFile(path, 'utf8');
  const route = routeOf(path);
  htmls.set(route, html);
  assert.match(html, /<html lang="zh-CN">/, `${route}: Chinese document language`);
  assert.equal([...html.matchAll(/<h1(?:\s|>)/g)].length, 1, `${route}: exactly one H1`);
  assert.match(html, /<title>[^<]+Serylane[^<]*<\/title>|<title>Serylane[^<]*<\/title>/, `${route}: title`);
  // Published asset and repository URLs are stable compatibility identities;
  // page copy, metadata and accessible names use only the current brand.
  const brandedContent = html.replace(/https?:\/\/[^\s"'<>]+/g, '');
  assert.doesNotMatch(brandedContent, /RouteDeck|mihomo-codex/i, `${route}: no former-brand copy outside URLs`);
  if (route !== '/404') assert.match(html, /<meta name="description" content="[^"]{20,}"/, `${route}: description`);
  if (route === '/404') assert.match(html, /content="noindex[^\"]*"/, '404 must not be indexed');
  else assert.ok(html.includes(`rel="canonical" href="${origin}${route}"`), `${route}: exact canonical`);
  assert.doesNotMatch(html, /<iframe|onclick=|onerror=|<form\b|<script[^>]+src="https?:/i, `${route}: static and local only`);
  const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]);
  assert.equal(ids.length, new Set(ids).size, `${route}: duplicate IDs`);
  for (const match of html.matchAll(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/g)) {
    const schema = JSON.parse(match[1]);
    assert.equal(schema.url, origin + route);
    assert.equal(schema.name, 'Serylane');
    assert.ok(!schema.alternateName, 'Do not advertise former brand aliases');
    assert.ok(!schema.aggregateRating && !schema.review, 'Do not fabricate ratings');
  }
}
for (const [route, html] of htmls) {
  for (const match of html.matchAll(/\b(?:href|src)="([^"]+)"/g)) {
    const href = match[1];
    if (/^(https?:|mailto:)/.test(href)) continue;
    const target = new URL(href, origin + route);
    if (/^\/download\/(windows|macos|linux)-(x64|arm64)$/.test(target.pathname)) {
      assert.ok(!target.search || target.search === '?channel=github');
      checkedLinks++;
      continue;
    }
    const plainRoute = target.pathname.replace(/index\.html$/, '').replace(/\.html$/, '');
    let targetHtml = htmls.get(plainRoute);
    if (!targetHtml) {
      const path = resolve(root, '.' + target.pathname);
      assert.ok(path.startsWith(resolve(root) + sep));
      assert.ok((await stat(path).catch(() => null))?.isFile(), `${route}: missing ${href}`);
    }
    if (target.hash && targetHtml) assert.ok(targetHtml.includes(`id="${decodeURIComponent(target.hash.slice(1))}"`), `${route}: broken anchor ${href}`);
    checkedLinks++;
  }
}
assert.match(htmls.get('/docs/'), /v0\.7\.16：三平台应用快选与更新跟随/);
assert.match(htmls.get('/docs/'), /NSWorkspace/);
assert.match(htmls.get('/docs/'), /Flatpak\/Snap/);
assert.match(htmls.get('/'), /缓存清单先显示、后台刷新不打断编辑/);
assert.match(htmls.get('/docs/'), /v0\.7\.9：更清晰的 macOS 双行网速/);
assert.match(htmls.get('/'), /macOS 原生双行网速/);
const sitemap = await readFile(join(root, 'sitemap.xml'), 'utf8');
for (const route of htmls.keys()) if (route !== '/404') assert.ok(sitemap.includes(`<loc>${origin}${route}</loc>`), `Sitemap omits ${route}`);
assert.ok(!(sitemap.includes('/404')));
assert.match(await readFile(join(root, 'robots.txt'), 'utf8'), /Sitemap: https:\/\/serylane\.cmmuu\.com\/sitemap\.xml/);
const script = await readFile(join(root, 'site.js'), 'utf8');
assert.doesNotMatch(script, /\b(?:XMLHttpRequest|WebSocket|invoke|EventSource)\s*\(|127\.0\.0\.1|localhost|__TAURI__|localStorage|sessionStorage/, 'Website must not contact proxy, storage or desktop services');
assert.equal([...script.matchAll(/\bfetch\s*\(/g)].length, 1);
assert.match(script, /fetch\("\/api\/releases\/latest",/);
assert.doesNotMatch(script, /version:\s*["']v\d+\./, 'No pinned release snapshot');
const config = JSON.parse(await readFile(new URL('../wrangler.jsonc', import.meta.url), 'utf8'));
assert.deepEqual(config.routes, [{ pattern: 'serylane.cmmuu.com', custom_domain: true }]);
assert.equal(config.assets.not_found_handling, '404-page');
assert.equal(config.workers_dev, false);
assert.deepEqual(config.assets.run_worker_first, ['/api/*', '/download/*']);
assert.equal(config.assets.binding, 'ASSETS');
console.log(`PASS: ${htmls.size} HTML pages, ${checkedLinks} local links/assets, canonical URLs, sitemap, JSON-LD, safety and deployment scope.`);
