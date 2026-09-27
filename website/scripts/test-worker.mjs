import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';
import { handle } from '../src/worker.ts';
import { TARGETS, GITHUB, GITEE, HK_FILES } from '../src/releases.ts';

const hash = text => createHash('sha256').update(text).digest('hex');
const suffix = { 'windows-x64': 'x64-setup.exe', 'windows-arm64': 'arm64-setup.exe', 'macos-x64': 'x64.dmg', 'macos-arm64': 'aarch64.dmg', 'linux-x64': 'amd64.AppImage', 'linux-arm64': 'aarch64.AppImage' };
export function fixture(version = 'v0.7.7') {
  const assets = Object.fromEntries(TARGETS.map((target, index) => [target, {
    filename: `${version === 'v0.7.6' ? 'RouteDeck' : 'Serylane'}_${version.slice(1)}_${suffix[target]}`,
    size: 1000 + index, sha256: hash(target),
  }]));
  const marker = version === 'v0.7.6' ? 'latest.json' : 'downloads.json';
  const markerText = JSON.stringify(marker === 'downloads.json' ? { schemaVersion: 1, version, assets } : {
    version: version.slice(1), platforms: Object.fromEntries(TARGETS.filter(t => !t.startsWith('macos')).map(t => [
      t.replace('-x64', '-x86_64').replace('-arm64', '-aarch64'), { ...assets[t], signature: 'existing-published-signature' },
    ])),
  });
  const sums = [...Object.values(assets).map(a => `${a.sha256}  ${a.filename}`), `${hash(markerText)}  ${marker}`].join('\n') + '\n';
  return { version, assets, marker, markerText, sums };
}
const env = { ASSETS: { fetch: () => new Response('static-assets') } };
function upstream(data, override = () => undefined) {
  const calls = [];
  const fetcher = async (url, init) => {
    calls.push({ url, method: init.method });
    assert.equal(init.redirect, 'manual');
    assert.equal(init.cache, 'no-store');
    assert.ok(init.signal);
    assert.ok(!init.headers.Authorization && !init.headers.Cookie);
    const altered = await override(url, init, calls);
    if (altered) return altered;
    if (url.startsWith(HK_FILES)) return new Response('not mirrored yet', { status: 404 });
    if (url === `${GITHUB}/releases/latest`) return new Response(null, { status: 302, headers: { location: `${GITHUB}/releases/tag/${data.version}` } });
    if (url.endsWith(`/${data.marker}`)) return new Response(data.markerText);
    if (url.endsWith('/SHA256SUMS.txt')) return new Response(data.sums);
    const asset = Object.values(data.assets).find(a => url.endsWith('/' + a.filename));
    if (asset && init.method === 'HEAD') return new Response(null, { headers: { 'content-length': String(asset.size), 'content-type': 'application/octet-stream' } });
    return new Response('missing', { status: 404 });
  };
  return { fetcher, calls };
}
const request = (path, method = 'GET') => new Request(`https://serylane.cmmuu.com${path}`, { method });
function noCache(response) {
  assert.match(response.headers.get('cache-control'), /no-store/);
  assert.equal(response.headers.get('cdn-cache-control'), 'no-store');
  assert.equal(response.headers.get('x-robots-tag'), 'noindex');
}

test('six platforms: latest exact packages, Gitee primary and GitHub override, no binary GET', async () => {
  const data = fixture();
  for (const target of TARGETS) for (const githubOnly of [false, true]) {
    const { fetcher, calls } = upstream(data);
    const response = await handle(request(`/download/${target}${githubOnly ? '?channel=github' : ''}`), env, fetcher);
    assert.equal(response.status, 302);
    assert.equal(response.headers.get('location'), `${githubOnly ? GITHUB : GITEE}/releases/download/${data.version}/${data.assets[target].filename}`);
    noCache(response);
    assert.ok(calls.every(c => c.method === 'HEAD' || /\/(downloads.json|SHA256SUMS.txt)$/.test(c.url)));
    if (githubOnly) assert.ok(calls.every(c => !c.url.startsWith(GITEE)));
  }
});
test('legacy published 0.7.6 works without rewriting or renaming its assets', async () => {
  const data = fixture('v0.7.6');
  const { fetcher } = upstream(data);
  const response = await handle(request('/api/releases/latest'), env, fetcher);
  assert.equal(response.status, 200);
  const result = await response.json();
  assert.equal(result.version, 'v0.7.6');
  assert.equal(Object.keys(result.assets).length, 6);
  assert.ok(Object.values(result.assets).every(a => a.domesticAvailable && a.filename.startsWith('RouteDeck_')));
});
test('legacy Windows download does not depend on unrelated macOS/Linux package HEAD requests', async () => {
  const { fetcher, calls } = upstream(fixture('v0.7.6'), url => /\.(dmg|AppImage)$/.test(url) ? new Response(null, { status: 503 }) : undefined);
  const result = await handle(request('/download/windows-x64'), env, fetcher);
  assert.equal(result.status, 302);
  assert.ok(calls.every(c => !/\.(dmg|AppImage)$/.test(c.url)));
});
test('mirror missing, timeout, HTML login, wrong bytes or wrong package size => SAME-version GitHub', async () => {
  for (const failure of ['missing', 'timeout', 'html', 'bytes', 'size']) {
    const data = fixture();
    const { fetcher } = upstream(data, (url, init) => {
      if (!url.startsWith(GITEE)) return;
      if (failure === 'timeout') throw new DOMException('timeout', 'TimeoutError');
      if (failure === 'missing') return new Response('missing', { status: 404 });
      if (failure === 'html') return new Response('<html>login</html>');
      if (failure === 'bytes') return new Response(data.markerText + ' ');
      if (init.method === 'HEAD') return new Response(null, { headers: { 'content-length': '50' } });
    });
    const response = await handle(request('/download/windows-arm64'), env, fetcher);
    assert.equal(response.status, 302, failure);
    assert.equal(response.headers.get('location'), `${GITHUB}/releases/download/v0.7.7/Serylane_0.7.7_arm64-setup.exe`);
  }
});
test('metadata reports partial mirrors without claiming all platforms ready', async () => {
  const { fetcher } = upstream(fixture(), (url, init) => {
    if (url.startsWith(GITEE) && init.method === 'HEAD' && !url.endsWith('_x64-setup.exe')) return new Response(null, { status: 404 });
  });
  const response = await handle(request('/api/releases/latest'), env, fetcher);
  const data = await response.json();
  assert.equal(response.status, 200);
  assert.deepEqual(Object.entries(data.assets).filter(([, a]) => a.domesticAvailable).map(([t]) => t), ['windows-x64']);
  assert.ok(!JSON.stringify(data).includes('https://'), 'No upstream URLs exposed as frontend inputs');
  noCache(response);
});
test('fresh resolution on every click; page-open release does not pin subsequent downloads', async () => {
  for (const version of ['v0.7.7', 'v0.7.8']) {
    const response = await handle(request('/download/linux-x64'), env, upstream(fixture(version)).fetcher);
    assert.equal(response.headers.get('x-serylane-version'), version);
    assert.ok(response.headers.get('location').includes(`/${version}/`));
  }
});
test('latest unavailable, prerelease, missing/corrupt manifest or unavailable binaries => 503, never older version', async () => {
  for (const failure of ['latest', 'prerelease', 'manifest', 'checksum', 'binary', 'oversize', 'streamOversize']) {
    const data = fixture();
    const { fetcher, calls } = upstream(data, (url, init) => {
      if (failure === 'latest' && url.endsWith('/latest')) return new Response(null, { status: 429 });
      if (failure === 'prerelease' && url.endsWith('/latest')) return new Response(null, { status: 302, headers: { location: `${GITHUB}/releases/tag/v0.8.0-beta.1` } });
      if (failure === 'manifest' && url.endsWith('/downloads.json')) return new Response(null, { status: 404 });
      if (failure === 'checksum' && url.endsWith('/SHA256SUMS.txt')) return new Response(data.sums.replace(hash(data.markerText), 'a'.repeat(64)));
      if (failure === 'oversize' && url.endsWith('/downloads.json')) return new Response('{}', { headers: { 'content-length': '500000' } });
      if (failure === 'streamOversize' && url.endsWith('/downloads.json')) return new Response('x'.repeat(262145));
      if (failure === 'binary' && init.method === 'HEAD' && !url.endsWith('/latest')) return new Response(null, { status: 404 });
    });
    const response = await handle(request('/download/windows-x64'), env, fetcher);
    assert.equal(response.status, 503, failure);
    assert.equal(response.headers.get('location'), null);
    assert.match(await response.text(), /本次没有转向旧版本/);
    noCache(response);
    assert.ok(calls.every(c => !c.url.includes('/v0.7.6/')));
  }
});
test('malformed future catalog is rejected even with matching catalog checksum', async () => {
  for (const mutation of [d => delete d.assets['macos-x64'], d => { d.assets['windows-x64'].filename = '../../other.exe'; }, d => { d.assets['linux-x64'].sha256 = 'b'.repeat(64); }, d => { d.version = 'v9.0.0'; }]) {
    const data = fixture();
    const json = JSON.parse(data.markerText);
    mutation(json);
    const oldHash = hash(data.markerText);
    data.markerText = JSON.stringify(json);
    data.sums = data.sums.replace(oldHash, hash(data.markerText));
    assert.equal((await handle(request('/api/releases/latest'), env, upstream(data).fetcher)).status, 503);
  }
});
test('upstream redirects cannot escape fixed HTTPS provider/CDN allowlists', async () => {
  for (const location of ['http://127.0.0.1:7890/', 'https://evil.example/file', 'https://foruda.gitee.com.evil.example/attach_file/x', 'https://user:secret@github.com/CMMUU/serylane/releases/tag/v0.7.7']) {
    const { fetcher, calls } = upstream(fixture(), url => url.endsWith('/downloads.json') && url.startsWith(GITHUB)
      ? new Response(null, { status: 302, headers: { location } }) : undefined);
    assert.equal((await handle(request('/api/releases/latest'), env, fetcher)).status, 503);
    assert.ok(!calls.some(c => c.url === location));
  }
});
test('known release CDN redirects work, loops fail bounded', async () => {
  const data = fixture();
  for (const loop of [false, true]) {
    const { fetcher, calls } = upstream(data, url => {
      if (url.startsWith(GITHUB) && url.endsWith('/downloads.json')) return new Response(null, { status: 302, headers: { location: loop ? url : 'https://release-assets.githubusercontent.com/github-production-release-asset/1355770287/catalog' } });
      if (url.includes('/github-production-release-asset/')) return new Response(data.markerText);
    });
    assert.equal((await handle(request('/api/releases/latest'), env, fetcher)).status, loop ? 503 : 200);
    assert.ok(calls.length < 30);
  }
});
test('invalid routes/queries/methods perform no outbound requests; assets passed through', async () => {
  const forbidden = () => assert.fail('Unexpected network');
  for (const [path, method, status] of [
    ['/download/windows-ia32', 'GET', 404], ['/api/unknown', 'GET', 404],
    ['/download/windows-x64?url=https://evil.example', 'GET', 400], ['/api/releases/latest?version=v1.0.0', 'GET', 400],
    ['/download/windows-x64?channel=github&channel=gitee', 'GET', 400], ['/download/windows-x64', 'POST', 405],
  ]) assert.equal((await handle(request(path, method), env, forbidden)).status, status);
  assert.equal(await (await handle(request('/docs/'), env, forbidden)).text(), 'static-assets');
});
test('HEAD download resolves normally but has no response body', async () => {
  const response = await handle(request('/download/windows-x64', 'HEAD'), env, upstream(fixture()).fetcher);
  assert.equal(response.status, 302);
  assert.equal(await response.text(), '');
});

function hkUpstream(data, failure) {
  return upstream(data, (url, init) => {
    if (!url.startsWith(HK_FILES)) return;
    if (failure === 'timeout') throw new DOMException('timeout', 'TimeoutError');
    if (failure === 'missing') return new Response(null, { status: 404 });
    if (url.endsWith('/' + data.marker)) return new Response(failure === 'stale' ? fixture('v0.7.6').markerText
      : failure === 'changed' ? data.markerText + ' ' : data.markerText);
    const asset = Object.values(data.assets).find(a => url.endsWith('/' + a.filename));
    if (asset && init.method === 'HEAD') return new Response(null, { headers: {
      'content-length': String(failure === 'size' ? asset.size + 1 : asset.size), 'content-type': 'application/octet-stream',
    } });
  });
}
test('HK archive is preferred for all six exact current packages, GitHub override skips both mirrors', async () => {
  const data = fixture();
  for (const target of TARGETS) {
    const { fetcher, calls } = hkUpstream(data);
    const response = await handle(request(`/download/${target}`), env, fetcher);
    assert.equal(response.status, 302);
    assert.equal(response.headers.get('location'), `${HK_FILES}/${data.version}/${data.assets[target].filename}`);
    assert.equal(response.headers.get('x-serylane-channel'), 'hk');
    assert.ok(calls.every(c => c.method === 'HEAD' || /\/(downloads.json|SHA256SUMS.txt)$/.test(c.url)));
    const override = hkUpstream(data);
    const github = await handle(request(`/download/${target}?channel=github`), env, override.fetcher);
    assert.equal(github.headers.get('x-serylane-channel'), 'github');
    assert.ok(override.calls.every(c => !c.url.startsWith(HK_FILES) && !c.url.startsWith(GITEE)));
  }
});
test('HK lag, timeout, missing file, changed marker or wrong length falls back only to identical current Gitee', async () => {
  for (const failure of ['stale', 'timeout', 'missing', 'changed', 'size']) {
    const { fetcher } = hkUpstream(fixture(), failure);
    const response = await handle(request('/download/macos-arm64'), env, fetcher);
    assert.equal(response.status, 302, failure);
    assert.equal(response.headers.get('location'), `${GITEE}/releases/download/v0.7.7/Serylane_0.7.7_aarch64.dmg`);
  }
});
test('HK catalog readiness is reported separately and does not conceal failed Gitee', async () => {
  const data = fixture();
  const hk = hkUpstream(data);
  const response = await handle(request('/api/releases/latest'), env, async (url, init) =>
    url.startsWith(GITEE) ? new Response(null, { status: 503 }) : hk.fetcher(url, init));
  assert.equal(response.status, 200);
  assert.equal(response.headers.get('x-serylane-hk'), 'VERIFIED');
  assert.notEqual(response.headers.get('x-serylane-mirror'), 'VERIFIED');
  assert.ok(Object.values((await response.json()).assets).every(a => a.domesticAvailable && a.hkAvailable && !a.giteeAvailable));
});
test('HK redirect allowlist rejects other origins, credentials, paths and query strings before request', async () => {
  for (const location of ['https://files.cmmuu.com.evil.example/releases/serylane/v0.7.7/downloads.json',
    'https://files.cmmuu.com/manual/other', 'https://files.cmmuu.com/releases/other/v0.7.7/downloads.json',
    'https://files.cmmuu.com/releases/serylane/v0.7.7/downloads.json?url=other',
    'https://files.cmmuu.com/releases/serylane/v0.7.6/downloads.json',
    'https://files.cmmuu.com/releases/serylane/v0.7.7/other.json',
    'http://files.cmmuu.com/releases/serylane/v0.7.7/downloads.json',
    'https://user@files.cmmuu.com/releases/serylane/v0.7.7/downloads.json']) {
    const data = fixture();
    const { fetcher, calls } = upstream(data, url => url.startsWith(HK_FILES)
      ? new Response(null, { status: 302, headers: { location } }) : undefined);
    const response = await handle(request('/download/windows-x64'), env, fetcher);
    assert.equal(response.headers.get('x-serylane-channel'), 'gitee');
    assert.ok(!calls.some(c => c.url === location), location);
  }
});
