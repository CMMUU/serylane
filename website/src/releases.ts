// Fixed upstreams only. Never accept a host, asset URL, version or filename from a visitor.
export const GITHUB = 'https://github.com/CMMUU/serylane';
export const GITEE = 'https://gitee.com/cmmuu/serylane';
export const HK_FILES = 'https://files.cmmuu.com/releases/serylane';
export const TARGETS = ['windows-x64', 'windows-arm64', 'macos-x64', 'macos-arm64', 'linux-x64', 'linux-arm64'] as const;
export type Target = typeof TARGETS[number];
export type Channel = 'github' | 'gitee' | 'hk';
export type Fetcher = (url: string, init: RequestInit) => Promise<Response>;
export type Asset = { filename: string; size: number; sha256: string };
export type Catalog = { version: string; assets: Partial<Record<Target, Asset>>; marker: string; markerText: string };
const STABLE = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const HASH = /^[a-f0-9]{64}$/;
const LIMIT = 256 * 1024;
const REDIRECTS = new Set([301, 302, 303, 307, 308]);

export class ReleaseError extends Error {
  code: string;
  constructor(code: string) { super(code); this.code = code; }
}
function requireValid(value: unknown): asserts value {
  if (!value) throw new ReleaseError('INVALID_RELEASE_METADATA');
}
function record(value: unknown): Record<string, unknown> {
  requireValid(value && typeof value === 'object' && !Array.isArray(value));
  return value as Record<string, unknown>;
}
export function assetURL(channel: Channel, version: string, filename: string): string {
  return channel === 'hk' ? `${HK_FILES}/${version}/${filename}`
    : `${channel === 'gitee' ? GITEE : GITHUB}/releases/download/${version}/${filename}`;
}
function allowed(url: URL, channel: Channel): boolean {
  if (url.protocol !== 'https:' || url.username || url.password || url.port || url.hash) return false;
  if (channel === 'hk') return url.origin === new URL(HK_FILES).origin && !url.search
    && /^\/releases\/serylane\/v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\/[A-Za-z0-9_.-]+$/.test(url.pathname);
  // Provider-owned release CDN destinations; no wildcard hostnames or user-controlled redirect endpoints.
  if (channel === 'github') return (url.origin === 'https://github.com' && url.pathname.startsWith('/CMMUU/serylane/releases/'))
    || (url.hostname === 'release-assets.githubusercontent.com' && url.pathname.startsWith('/github-production-release-asset/'));
  return (url.origin === 'https://gitee.com' && (url.pathname.startsWith('/cmmuu/serylane/releases/download/')
    || /^\/cmmuu\/serylane\/attach_files\/\d+\/download\//.test(url.pathname)))
    || (url.hostname === 'foruda.gitee.com' && url.pathname.startsWith('/attach_file/'));
}
async function dispose(response: Response) { await response.body?.cancel(); }
async function upstream(fetcher: Fetcher, input: string, channel: Channel, method: 'HEAD' | 'GET', signal: AbortSignal): Promise<Response> {
  let url = new URL(input);
  for (let hop = 0; hop < 5; hop++) {
    requireValid(allowed(url, channel));
    const response = await fetcher(url.href, {
      method, redirect: 'manual', signal, cache: 'no-store',
      headers: { 'User-Agent': 'Serylane-Downloads/1.0', 'Cache-Control': 'no-cache', Accept: '*/*' },
    }).catch(() => { throw new ReleaseError(`${channel.toUpperCase()}_${method}_UNAVAILABLE`); });
    if (!REDIRECTS.has(response.status)) return response;
    const location = response.headers.get('location');
    await dispose(response);
    requireValid(location);
    const destination = new URL(location, url);
    // Our file gateway serves immutable paths directly. A redirect must not
    // silently switch a tag/filename (even to another allowed archive path).
    if (channel === 'hk') requireValid(destination.href === input);
    url = destination;
  }
  throw new ReleaseError('UPSTREAM_REDIRECT_LIMIT');
}
export async function boundedText(response: Response): Promise<string> {
  if (response.status !== 200) {
    await dispose(response);
    throw new ReleaseError(`METADATA_HTTP_${response.status}`);
  }
  if (Number(response.headers.get('content-length')) > LIMIT) {
    await dispose(response);
    throw new ReleaseError('RELEASE_METADATA_TOO_LARGE');
  }
  requireValid(response.body);
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      length += value.length;
      if (length > LIMIT) throw new ReleaseError('RELEASE_METADATA_TOO_LARGE');
      chunks.push(value);
    }
  } finally { await reader.cancel(); reader.releaseLock(); }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
  return new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes);
}
async function metadata(fetcher: Fetcher, channel: Channel, version: string, filename: string, signal: AbortSignal) {
  return boundedText(await upstream(fetcher, assetURL(channel, version, filename), channel, 'GET', signal));
}
async function digest(text: string) {
  const result = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text));
  return [...new Uint8Array(result)].map(byte => byte.toString(16).padStart(2, '0')).join('');
}
function checksums(text: string): Map<string, string> {
  const result = new Map<string, string>();
  for (const line of text.trim().split('\n')) {
    const match = /^([a-f0-9]{64})  ([A-Za-z0-9_.-]+)\r?$/.exec(line);
    requireValid(match && !result.has(match[2]));
    result.set(match[2], match[1]);
  }
  return result;
}
export function filenames(version: string): Record<Target, string> {
  const [major, minor, patch] = version.slice(1).split('.').map(Number);
  const brand = major === 0 && (minor < 7 || (minor === 7 && patch < 7)) ? 'RouteDeck' : 'Serylane';
  const prefix = `${brand}_${version.slice(1)}`;
  return {
    'windows-x64': `${prefix}_x64-setup.exe`, 'windows-arm64': `${prefix}_arm64-setup.exe`,
    'macos-x64': `${prefix}_x64.dmg`, 'macos-arm64': `${prefix}_aarch64.dmg`,
    'linux-x64': `${prefix}_amd64.AppImage`, 'linux-arm64': `${prefix}_aarch64.AppImage`,
  };
}
async function packageSize(fetcher: Fetcher, channel: Channel, version: string, filename: string, signal: AbortSignal): Promise<number> {
  const response = await upstream(fetcher, assetURL(channel, version, filename), channel, 'HEAD', signal);
  const size = Number(response.headers.get('content-length'));
  const type = response.headers.get('content-type') ?? '';
  await dispose(response);
  requireValid(response.status === 200 && Number.isSafeInteger(size) && size > 0
    && !/text\/|json|html/i.test(type));
  return size;
}

export async function latestCatalog(fetcher: Fetcher, requestedTargets: readonly Target[] = TARGETS): Promise<Catalog> {
  const signal = AbortSignal.timeout(12000);
  // Use GitHub's public latest-stable redirect instead of the anonymous REST quota.
  // Resolve on EVERY request; never fall back to a cached or older version.
  const latest = await fetcher(`${GITHUB}/releases/latest`, {
    method: 'HEAD', redirect: 'manual', signal, cache: 'no-store',
    headers: { 'User-Agent': 'Serylane-Downloads/1.0', 'Cache-Control': 'no-cache' },
  }).catch(() => { throw new ReleaseError('LATEST_LOOKUP_UNAVAILABLE'); });
  const location = latest.headers.get('location');
  await dispose(latest);
  requireValid(REDIRECTS.has(latest.status) && location);
  const url = new URL(location, GITHUB);
  requireValid(url.origin === 'https://github.com' && !url.search && !url.hash && !url.username && !url.password
    && url.pathname.startsWith('/CMMUU/serylane/releases/tag/'));
  const version = url.pathname.slice('/CMMUU/serylane/releases/tag/'.length);
  requireValid(version.length < 40 && STABLE.test(version));
  // Only the already-published v0.7.6 needs compatibility. Never infer readiness for a new release.
  const marker = version === 'v0.7.6' ? 'latest.json' : 'downloads.json';
  const [markerText, sumsText] = await Promise.all([
    metadata(fetcher, 'github', version, marker, signal),
    metadata(fetcher, 'github', version, 'SHA256SUMS.txt', signal),
  ]);
  const sums = checksums(sumsText);
  requireValid(sums.get(marker) === await digest(markerText));
  const manifest = record(JSON.parse(markerText));
  const expected = filenames(version);
  const assets = {} as Record<Target, Asset>;
  if (marker === 'downloads.json') {
    requireValid(manifest.schemaVersion === 1 && manifest.version === version);
    const rows = record(manifest.assets);
    requireValid(Object.keys(rows).length === TARGETS.length);
    for (const target of TARGETS) {
      const row = record(rows[target]);
      requireValid(row.filename === expected[target] && typeof row.sha256 === 'string' && HASH.test(row.sha256)
        && sums.get(expected[target]) === row.sha256 && typeof row.size === 'number' && Number.isSafeInteger(row.size) && row.size > 0);
      assets[target] = { filename: expected[target], size: row.size, sha256: row.sha256 };
    }
  } else {
    requireValid(manifest.version === version.slice(1));
    const platforms = record(manifest.platforms);
    await Promise.all(requestedTargets.map(async target => {
      const filename = expected[target];
      const hash = sums.get(filename);
      requireValid(hash);
      let size: number;
      if (target.startsWith('macos-')) size = await packageSize(fetcher, 'github', version, filename, signal);
      else {
        const row = record(platforms[target.replace('-x64', '-x86_64').replace('-arm64', '-aarch64')]);
        requireValid(row.sha256 === hash && typeof row.size === 'number' && Number.isSafeInteger(row.size) && row.size > 0
          && typeof row.signature === 'string' && row.signature.length > 0);
        size = row.size;
      }
      assets[target] = { filename, sha256: hash, size };
    }));
  }
  return { version, assets, marker, markerText };
}

export function catalogAsset(catalog: Catalog, target: Target): Asset {
  const asset = catalog.assets[target];
  requireValid(asset);
  return asset;
}

export async function mirrorReady(fetcher: Fetcher, catalog: Catalog, targets: readonly Target[], channel: 'gitee' | 'hk' = 'gitee'): Promise<{ ready: Set<Target>; diagnostic: string }> {
  const signal = AbortSignal.timeout(4000);
  try {
    // This marker is mirrored only after every package/signature/checksum has verified.
    // Compare exact authoritative bytes; a matching version string alone is insufficient.
    const marker = await metadata(fetcher, channel, catalog.version, catalog.marker, signal);
    if (marker !== catalog.markerText) throw new ReleaseError('MIRROR_MARKER_MISMATCH');
    const results = await Promise.all(targets.map(async target => {
      try {
        const asset = catalogAsset(catalog, target);
        const size = await packageSize(fetcher, channel, catalog.version, asset.filename, signal);
        return size === asset.size ? target : null;
      } catch { return null; }
    }));
    const ready = new Set(results.filter((target): target is Target => target !== null));
    return { ready, diagnostic: ready.size === targets.length ? 'VERIFIED' : 'PACKAGE_UNVERIFIED' };
  } catch (error) {
    console.info({ event: 'mirror_unavailable', channel, code: error instanceof ReleaseError ? error.code : 'MIRROR_METADATA_UNAVAILABLE' });
    return { ready: new Set(), diagnostic: error instanceof ReleaseError ? error.code : 'MIRROR_METADATA_UNAVAILABLE' };
  }
}

export async function resolveDownload(fetcher: Fetcher, target: Target, githubOnly: boolean) {
  const catalog = await latestCatalog(fetcher, [target]);
  const asset = catalogAsset(catalog, target);
  if (!githubOnly) {
    // Independent mirrors run within the same bound. HK never hides a newer
    // upstream release and is only eligible after matching the exact catalog.
    const [hk, gitee] = await Promise.all([
      mirrorReady(fetcher, catalog, [target], 'hk'), mirrorReady(fetcher, catalog, [target], 'gitee'),
    ]);
    const channel = hk.ready.has(target) ? 'hk' : gitee.ready.has(target) ? 'gitee' : null;
    if (channel) return { version: catalog.version, channel, url: assetURL(channel, catalog.version, asset.filename) };
  }
  const size = await packageSize(fetcher, 'github', catalog.version, asset.filename, AbortSignal.timeout(5000));
  requireValid(size === asset.size);
  return { version: catalog.version, channel: 'github', url: assetURL('github', catalog.version, asset.filename) };
}
