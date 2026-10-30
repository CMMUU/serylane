import assert from 'node:assert/strict';
import { test } from 'node:test';
import { handle } from '../src/worker.ts';
import { TARGETS, HK_FILES, filenames } from '../src/releases.ts';
import { CENTER, CENTER_API } from '../src/center.ts';
const env = { ASSETS: { fetch: () => new Response('static-assets') } };
const request = (path, method = 'GET') => new Request('https://serylane.cmmuu.com' + path, { method });
function fixture(version = 'v0.7.33') {
  const names = filenames(version);
  return { schemaVersion: 1, project: 'serylane', version, checkedAt: new Date().toISOString(), source: 'github',
    assets: Object.fromEntries(TARGETS.map((target, i) => [target, {
      filename: names[target], size: 1000 + i, sha256: 'a'.repeat(64), channel: 'hk',
      hkAvailable: true, domesticAvailable: true, giteeAvailable: false,
      fileUrl: `${HK_FILES}/${version}/${names[target]}`,
      downloadUrl: `${CENTER}/download/serylane/latest/${target}`,
    }])) };
}
function upstream(data, override = () => undefined) {
  const calls = [];
  const fetcher = async (url, init) => {
    calls.push({ url, method: init.method });
    assert.equal(init.redirect, 'manual');
    assert.equal(init.cache, 'no-store');
    assert.ok(init.signal);
    assert.ok(!init.headers.Authorization && !init.headers.Cookie);
    const altered = await override(url, init);
    if (altered) return altered;
    if (url === CENTER_API) return Response.json(data);
    const asset = Object.values(data.assets).find(a => url === a.fileUrl);
    assert.ok(asset, 'No alternate source may be called');
    assert.equal(init.method, 'HEAD', 'Never download installer bodies in the website worker');
    return new Response(null, { headers: { 'content-length': String(asset.size), 'content-type': 'application/octet-stream' } });
  };
  return { fetcher, calls };
}
function noCache(response) {
  assert.match(response.headers.get('cache-control'), /no-store/);
  assert.equal(response.headers.get('cdn-cache-control'), 'no-store');
  assert.equal(response.headers.get('x-robots-tag'), 'noindex');
}
test('six center-only routes and resolve APIs preserve version and exact architecture; provenance is not a download channel', async () => {
  const data = fixture();
  for (const target of TARGETS) for (const method of ['GET', 'HEAD']) for (const prefix of ['/download/', '/api/download/']) {
    const { fetcher, calls } = upstream(data);
    const reply = await handle(request(prefix + target, method), env, fetcher);
    const api = prefix.startsWith('/api');
    assert.equal(reply.status, api ? 200 : 302);
    assert.equal(reply.headers.get('x-serylane-version'), data.version);
    assert.equal(reply.headers.get('x-serylane-channel'), 'hk');
    assert.equal(reply.headers.get('x-serylane-resolver'), 'download-center');
    if (api && method === 'GET') assert.deepEqual(await reply.json(), { version: data.version, channel: 'hk', url: data.assets[target].fileUrl });
    else if (!api) assert.equal(reply.headers.get('location'), data.assets[target].fileUrl);
    if (method === 'HEAD') assert.equal(await reply.text(), '');
    noCache(reply);
    assert.deepEqual(calls, [{ url: CENTER_API, method: 'GET' }, { url: data.assets[target].fileUrl, method: 'HEAD' }]);
  }
});
test('catalog is refreshed each request; source publishing a new version never uses a stored snapshot', async () => {
  for (const version of ['v0.7.33', 'v0.7.34']) {
    const data = fixture(version);
    const { fetcher, calls } = upstream(data);
    const reply = await handle(request('/api/releases/latest'), env, fetcher);
    assert.equal(reply.status, 200);
    assert.equal((await reply.json()).version, version);
    assert.deepEqual(calls, [{ url: CENTER_API, method: 'GET' }]);
    noCache(reply);
  }
});
test('historical archive names are not rewritten', async () => {
  const data = fixture('v0.7.6');
  const { fetcher } = upstream(data);
  const reply = await handle(request('/download/windows-x64'), env, fetcher);
  assert.equal(reply.headers.get('location'), HK_FILES + '/v0.7.6/RouteDeck_0.7.6_x64-setup.exe');
});
test('retired channels, version overrides and unknown query parameters fail before networking', async () => {
  const fetcher = () => assert.fail('Invalid request must not fetch');
  for (const path of ['/download/windows-x64', '/api/download/windows-x64', '/api/releases/latest']) {
    for (const query of ['?channel=github', '?channel=gitee', '?channel=hk', '?version=v0.7.33', '?url=https://evil.example', '?channel=github&channel=hk']) {
      const reply = await handle(request(path + query), env, fetcher);
      assert.equal(reply.status, 400);
      noCache(reply);
    }
  }
});
test('invalid routes and methods fail locally; static assets are delegated', async () => {
  const fetcher = () => assert.fail('Must not fetch');
  assert.equal(await (await handle(request('/docs/'), env, fetcher)).text(), 'static-assets');
  for (const path of ['/download/windows-x86', '/download/windows-x64/extra', '/api/nope']) assert.equal((await handle(request(path), env, fetcher)).status, 404);
  const post = await handle(request('/download/windows-x64', 'POST'), env, fetcher);
  assert.equal(post.status, 405);
  assert.equal(post.headers.get('allow'), 'GET, HEAD');
  assert.equal(await (await handle(request('/api/nope', 'HEAD'), env, fetcher)).text(), '');
});
test('center failures produce retryable 503, no other source and no sensitive details', async () => {
  for (const failure of ['network', 'redirect', 'status', 'malformed', 'oversized-header', 'oversized-body']) {
    for (const path of ['/download/windows-x64', '/api/download/windows-x64', '/api/releases/latest']) {
      const calls = [];
      const fetcher = async (url, init) => {
        calls.push(url);
        assert.equal(url, CENTER_API);
        assert.equal(init.redirect, 'manual');
        if (failure === 'network') throw new Error('secret-token https://evil.example/private');
        if (failure === 'redirect') return new Response(null, { status: 302, headers: { location: 'https://github.com/CMMUU/serylane/releases/latest' } });
        if (failure === 'status') return new Response('secret-token', { status: 503 });
        if (failure === 'malformed') return new Response('secret-token');
        if (failure === 'oversized-header') return new Response('{}', { headers: { 'content-length': String(256 * 1024 + 1) } });
        return new Response(' '.repeat(256 * 1024 + 1));
      };
      const reply = await handle(request(path), env, fetcher);
      assert.equal(reply.status, 503, failure);
      assert.equal(reply.headers.get('retry-after'), '30');
      noCache(reply);
      const body = await reply.text();
      assert.match(body, /重试/);
      assert.doesNotMatch(body, /github|gitee|secret-token|evil\.example/i);
      assert.deepEqual(calls, [CENTER_API]);
    }
  }
});
test('reject malformed, stale, wrong-architecture and foreign-source metadata without fallback', async () => {
  const mutations = [
    d => { d.version = 'v0.7.34-beta.1'; },
    d => { d.version = 'v00.7.34'; },
    d => { d.checkedAt = new Date(Date.now() - 61000).toISOString(); },
    d => { d.checkedAt = new Date(Date.now() + 61000).toISOString(); },
    d => { d.checkedAt = 'invalid'; },
    d => { d.project = 'other'; },
    d => { d.schemaVersion = 2; },
    d => { delete d.assets['macos-x64']; },
    d => { d.assets.other = {}; },
    ...['filename', 'sha256', 'size'].map(field => d => { d.assets['windows-x64'][field] = field === 'size' ? -1 : 'incorrect'; }),
    d => { d.assets['windows-x64'].size = 2 * 1024 ** 3 + 1; },
    d => { d.assets['windows-x64'].size = 1.5; },
    d => { d.assets['macos-x64'].filename = d.assets['macos-arm64'].filename; },
    d => { d.assets['windows-x64'].channel = 'github'; d.assets['windows-x64'].fileUrl = 'https://github.com/CMMUU/serylane/releases/download/v0.7.33/Serylane_0.7.33_x64-setup.exe'; },
    d => { d.assets['windows-x64'].channel = 'gitee'; },
    d => { d.assets['windows-x64'].hkAvailable = false; },
    d => { d.assets['windows-x64'].giteeAvailable = true; },
    d => { d.assets['windows-x64'].downloadUrl += '?channel=github'; },
    ...['https://evil.example/package.exe', 'https://files.cmmuu.com.evil.example/package.exe',
      'https://user:password@files.cmmuu.com/releases/serylane/v0.7.33/Serylane_0.7.33_x64-setup.exe',
      HK_FILES + '/v0.7.32/Serylane_0.7.32_x64-setup.exe',
      HK_FILES + '/v0.7.33/Serylane_0.7.33_arm64-setup.exe',
      HK_FILES + '/v0.7.33/Serylane_0.7.33_x64-setup.exe?channel=github',
      HK_FILES + '/v0.7.33/Serylane_0.7.33_x64-setup.exe#fragment'].map(url => d => { d.assets['windows-x64'].fileUrl = url; }),
  ];
  for (const mutate of mutations) {
    const data = fixture(); mutate(data);
    const { fetcher, calls } = upstream(data);
    const reply = await handle(request('/download/windows-x64'), env, fetcher);
    assert.equal(reply.status, 503, mutate.toString());
    assert.deepEqual(calls, [{ url: CENTER_API, method: 'GET' }]);
  }
});
test('archive HEAD must confirm exact size and non-HTML file, with no redirects', async () => {
  const data = fixture();
  const bad = [
    new Response(null, { status: 404 }),
    new Response(null, { status: 302, headers: { location: 'https://github.com/CMMUU/serylane/releases/download/v0.7.33/Serylane_0.7.33_x64-setup.exe' } }),
    new Response(null, { status: 302, headers: { location: data.assets['windows-arm64'].fileUrl } }),
    new Response(null, { headers: { 'content-length': '999', 'content-type': 'application/octet-stream' } }),
    new Response(null, { headers: { 'content-length': '1000', 'content-type': 'text/html' } }),
    new Response(null, { headers: { 'content-length': '1000', 'content-type': 'application/json' } }),
    new Response(null, { headers: { 'content-type': 'application/octet-stream' } }),
    new Response(null, { headers: { 'content-length': '1000' } }),
    new Response(null, { headers: { 'content-length': '1000', 'content-type': 'application/octet-stream', location: data.assets['windows-arm64'].fileUrl } }),
  ];
  for (const response of bad) {
    const { fetcher, calls } = upstream(data, url => url === CENTER_API ? undefined : response);
    const reply = await handle(request('/download/windows-x64', 'HEAD'), env, fetcher);
    assert.equal(reply.status, 503);
    assert.equal(await reply.text(), '');
    assert.equal(calls.length, 2);
    assert.ok(calls.every(call => call.url === CENTER_API || call.url === data.assets['windows-x64'].fileUrl));
  }
});
