import type { Fetcher } from './releases.ts';

export const GITHUB_UPDATE_PATH = '/api/update/github/';
const GITHUB_RELEASES = 'https://github.com/CMMUU/serylane/releases';
const MANIFEST = 'latest-serylane.json';
const VERSION = /^(0|[1-9]\d{0,19})\.(0|[1-9]\d{0,19})\.(0|[1-9]\d{0,19})$/;
const ARTIFACT = /^(Serylane|RouteDeck)_([^_]+)_(x64-setup\.exe|arm64-setup\.exe|x64_en-US\.msi|arm64_en-US\.msi|x64\.app\.tar\.gz|aarch64\.app\.tar\.gz|amd64\.AppImage|aarch64\.AppImage)$/;
const CDN_HOSTS = new Set(['release-assets.githubusercontent.com', 'objects.githubusercontent.com']);
const REDIRECTS = new Set([301, 302, 303, 307, 308]);
const HEADERS = {
  'X-Content-Type-Options': 'nosniff',
  'X-Robots-Tag': 'noindex',
  'X-Serylane-Channel': 'github_accelerator',
  'Cache-Control': 'no-store',
};

export interface GithubUpdateCache {
  cache: Pick<Cache, 'match' | 'put'>;
  waitUntil: (promise: Promise<void>) => void;
}

async function requestHeaders(fetcher: Fetcher, url: string, method: string): Promise<Response> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 8000);
  try {
    return await fetcher(url, {
      method,
      redirect: 'manual',
      cache: 'no-store',
      signal: controller.signal,
      headers: { 'Accept-Encoding': 'identity' },
    });
  } finally {
    clearTimeout(timer);
  }
}

function redirect(response: Response): URL {
  const location = new URL(response.headers.get('location') ?? '');
  if (location.protocol !== 'https:' || location.port || location.username || location.password || location.hash) {
    throw new Error('invalid upstream redirect');
  }
  return location;
}

async function latest(fetcher: Fetcher): Promise<Response> {
  const response = await requestHeaders(fetcher, GITHUB_RELEASES + '/latest/download/' + MANIFEST, 'HEAD');
  if (!REDIRECTS.has(response.status)) throw new Error('no stable release');
  const url = redirect(response);
  const version = url.pathname.split('/').at(-2)?.slice(1) ?? '';
  const tag = 'v' + version;
  if (!VERSION.test(version) || url.href !== GITHUB_RELEASES + '/download/' + tag + '/' + MANIFEST) {
    throw new Error('unexpected release tag');
  }
  return Response.json({
    tag_name: tag, draft: false, prerelease: false, assets: [{ name: MANIFEST }],
  }, { headers: HEADERS });
}

async function openAsset(fetcher: Fetcher, version: string, name: string, method: string): Promise<Response> {
  const origin = GITHUB_RELEASES + '/download/v' + version + '/' + name;
  let response = await requestHeaders(fetcher, origin, method);
  for (let count = 0; REDIRECTS.has(response.status) && count < 3; count++) {
    const url = redirect(response);
    await response.body?.cancel();
    if (!CDN_HOSTS.has(url.hostname)) throw new Error('untrusted release storage');
    response = await requestHeaders(fetcher, url.href, method);
  }
  const size = Number(response.headers.get('content-length'));
  const limit = name === MANIFEST ? 512 * 1024 : 256 * 1024 * 1024;
  if (response.status !== 200 || !Number.isSafeInteger(size) || size <= 0 || size > limit) {
    await response.body?.cancel();
    throw new Error('upstream asset unavailable');
  }
  let body = response.body;
  if (method === 'HEAD') {
    await body?.cancel();
    body = null;
  } else {
    if (!body) throw new Error('upstream response omitted its body');
    if (typeof FixedLengthStream !== 'undefined') {
      // Workers compute Content-Length from the stream instead of the header.
      const exact = new FixedLengthStream(size);
      void body.pipeTo(exact.writable).catch(() => {});
      body = exact.readable;
    }
  }
  return new Response(body, {
    headers: {
      ...HEADERS,
      'Cache-Control': 'public, max-age=3600',
      'Content-Length': String(size),
      'Content-Type': name === MANIFEST ? 'application/json' : 'application/octet-stream',
    },
  });
}

export async function githubUpdate(
  request: Request,
  url: URL,
  fetcher: Fetcher,
  edge?: GithubUpdateCache,
): Promise<Response> {
  const error = (status: number) => new Response(null, { status, headers: HEADERS });
  if (url.search) return error(400);
  if (request.method !== 'GET' && request.method !== 'HEAD') {
    return new Response(null, { status: 405, headers: { ...HEADERS, Allow: 'GET, HEAD' } });
  }
  const path = url.pathname.slice(GITHUB_UPDATE_PATH.length);
  try {
    if (path === 'latest') {
      const result = await latest(fetcher);
      return request.method === 'HEAD' ? new Response(null, result) : result;
    }
    const [tag, name, extra] = path.split('/');
    const version = tag.slice(1);
    if (!tag.startsWith('v') || !VERSION.test(version) || name === undefined || extra !== undefined
      || (name !== MANIFEST && ARTIFACT.exec(name)?.[2] !== version)) {
      return error(404);
    }
    const key = 'https://serylane.cmmuu.com' + url.pathname;
    const cached = await edge?.cache.match(key).catch(() => undefined);
    if (cached) return request.method === 'HEAD' ? new Response(null, cached) : cached;
    const response = await openAsset(fetcher, version, name, request.method);
    if (edge && request.method === 'GET') {
      edge.waitUntil(edge.cache.put(key, response.clone()).catch(() => {}));
    }
    return response;
  } catch {
    return error(503);
  }
}
