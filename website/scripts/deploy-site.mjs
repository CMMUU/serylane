// Production updates intentionally never run `deploy` or `triggers deploy`:
// the existing custom-domain/DNS binding must not be rewritten.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const repository = 'CMMUU/serylane';
const origin = 'https://serylane.cmmuu.com';
const account = '97eb9abaee45ec161253c18426b7b51b';
const sha256 = data => createHash('sha256').update(data).digest('hex');
const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8', windowsHide: true }).trim();

export function uploadedVersion(output) {
  const matches = [...output.matchAll(/^Worker Version ID:\s*([a-f0-9-]{36})\s*$/gm)];
  assert.equal(matches.length, 1, 'Upload must return exactly one Worker version ID');
  assert.match(matches[0][1], /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/);
  return matches[0][1];
}

export function sourceAssetBytes(path, bytes) {
  if (path !== '/robots.txt') return bytes;
  // Cloudflare's existing managed robots policy prepends its own section.
  // Keep that policy enabled; compare our complete original file after it.
  const content = bytes.toString('utf8');
  const start = '# BEGIN Cloudflare Managed content\n';
  const end = '# END Cloudflare Managed Content\n\n';
  if (!content.includes(start)) return bytes;
  assert.ok(content.startsWith('# As a condition of accessing this website,'), 'Unexpected robots prefix');
  assert.equal(content.split(start).length, 2, 'Ambiguous managed robots start');
  assert.equal(content.split(end).length, 2, 'Ambiguous managed robots end');
  assert.ok(content.indexOf(end) > content.indexOf(start));
  return Buffer.from(content.slice(content.indexOf(end) + end.length));
}

async function request(url, options = {}) {
  return fetch(url, { redirect: 'manual', cache: 'no-store', signal: AbortSignal.timeout(30000), ...options });
}
async function currentMain() {
  const headers = { Accept: 'application/vnd.github+json', 'User-Agent': 'serylane-website-sync' };
  if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
  const response = await request(`https://api.github.com/repos/${repository}/git/ref/heads/main`, { headers });
  assert.equal(response.status, 200, 'Cannot confirm current trusted main');
  return (await response.json()).object.sha;
}
export async function verifyStatic(expected, fetcher = request) {
  const marker = await fetcher(`${origin}/build-info.json`);
  assert.equal(marker.status, 200, 'Live build marker missing');
  assert.match(marker.headers.get('cache-control') ?? '', /no-store/);
  assert.deepEqual(await marker.json(), expected, 'Live site does not match the requested source');
  for (const [path, hash] of Object.entries(expected.files)) {
    const response = await fetcher(origin + path);
    assert.equal(response.status, 200, `Live page unavailable: ${path}`);
    assert.equal(sha256(sourceAssetBytes(path, Buffer.from(await response.arrayBuffer()))), hash, `Live content differs: ${path}`);
  }
}
export async function verifyDownloads(fetcher = request) {
  const catalog = await fetcher(origin + '/api/releases/latest');
  assert.equal(catalog.status, 200, 'Latest release lookup failed; deployment is not fully verified');
  const release = await catalog.json();
  assert.match(release.version, /^v\d+\.\d+\.\d+$/);
  const targets = ['windows-x64', 'windows-arm64', 'macos-x64', 'macos-arm64', 'linux-x64', 'linux-arm64'];
  assert.deepEqual(Object.keys(release.assets).sort(), [...targets].sort());
  const bases = { github: 'https://github.com/CMMUU/serylane/releases/download',
    gitee: 'https://gitee.com/cmmuu/serylane/releases/download', hk: 'https://files.cmmuu.com/releases/serylane' };
  // Verify BOTH the primary selection and explicit fallback for all six targets.
  // HEAD only: deployment verification never pulls installer bodies into Node.
  for (const target of targets) for (const githubOnly of [false, true]) {
    const response = await fetcher(`${origin}/download/${target}${githubOnly ? '?channel=github' : ''}`, { method: 'HEAD' });
    assert.equal(response.status, 302, `Latest download not verified: ${target}`);
    assert.equal(response.headers.get('x-serylane-version'), release.version, 'Release changed during verification; rerun');
    const channel = response.headers.get('x-serylane-channel');
    assert.ok(Object.hasOwn(bases, channel), 'Unknown download channel');
    if (githubOnly) assert.equal(channel, 'github');
    assert.equal(response.headers.get('location'), `${bases[channel]}/${release.version}/${release.assets[target].filename}`,
      `Channel/version/architecture identity differs: ${target}`);
  }
  console.log(`Verified latest release ${release.version}, six primary routes and six GitHub fallback routes.`);
}
export async function retryRead(check, wait = () => new Promise(resolve => setTimeout(resolve, 15000))) {
  // Edge activation can briefly return the previous asset manifest. Retry only
  // reads for up to 75 seconds; never repeat an upload or deployment write.
  const attempts = 6;
  for (let attempt = 1; attempt <= attempts; attempt++) {
    try { return await check(); } catch (error) {
      if (attempt === attempts) throw error;
      console.log(`Live verification not ready; retry ${attempt}/${attempts - 1}.`);
      await wait();
    }
  }
}
export async function main() {
  assert.ok(!process.env.GITHUB_REPOSITORY || process.env.GITHUB_REPOSITORY === repository);
  assert.ok(!process.env.CLOUDFLARE_ACCOUNT_ID || process.env.CLOUDFLARE_ACCOUNT_ID === account);
  if (process.env.CI) assert.ok(process.env.CLOUDFLARE_API_TOKEN, 'Missing CLOUDFLARE_API_TOKEN: website was NOT deployed');
  assert.equal(git('status', '--porcelain', '--untracked-files=all'), '', 'Commit reviewed source before deploying');
  const commit = git('rev-parse', 'HEAD');
  assert.equal(commit, await currentMain(), 'Refusing to deploy a stale/non-main revision');
  const config = JSON.parse(await readFile(root + 'wrangler.jsonc', 'utf8'));
  assert.equal(config.name, 'serylane-website');
  assert.deepEqual(config.routes, [{ pattern: 'serylane.cmmuu.com', custom_domain: true }]);
  const files = {};
  for (const path of ['index.html', 'site.js', 'styles.css', 'docs/index.html', 'robots.txt', 'sitemap.xml']) {
    files[path === 'index.html' ? '/' : path === 'docs/index.html' ? '/docs/' : '/' + path] = sha256(await readFile(root + 'public/' + path));
  }
  const expected = { sourceCommit: commit, websiteTree: git('rev-parse', 'HEAD:website'), files };
  // Generated deployment artifact; ignored by Git, no credentials or user data.
  await writeFile(root + 'public/build-info.json', JSON.stringify(expected) + '\n');
  const run = args => {
    try {
      return execFileSync(process.execPath, [root + 'node_modules/wrangler/bin/wrangler.js', ...args], {
        cwd: root, encoding: 'utf8', windowsHide: true, timeout: 600000,
        env: { ...process.env, CLOUDFLARE_ACCOUNT_ID: account, WRANGLER_SEND_METRICS: 'false', NO_COLOR: '1' },
        stdio: ['ignore', 'pipe', 'pipe'],
      });
    } catch { throw new Error('Wrangler step failed; no automatic retry or DNS changes. Inspect Cloudflare deployment state before retrying.'); }
  };
  run(['versions', 'upload', '--dry-run']);
  console.log('Validated website bundle; uploading to the existing Worker.');
  const version = uploadedVersion(run(['versions', 'upload', '--keep-vars', '--message', `Website source ${commit}`]));
  console.log(`Uploaded Worker version ${version}; not yet serving traffic.`);
  assert.equal(commit, await currentMain(), 'Main changed during upload; version was NOT activated');
  run(['versions', 'deploy', `${version}@100`, '--yes', '--message', `Website source ${commit}`]);
  console.log(`Activated ${version}. Verifying public site before reporting success.`);
  await retryRead(() => verifyStatic(expected));
  await retryRead(verifyDownloads);
  console.log(`Website synchronized and verified: ${origin}, source ${commit}, Worker ${version}`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
