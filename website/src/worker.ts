import { TARGETS, ReleaseError } from './releases.ts';
import type { Fetcher, Target } from './releases.ts';
import { centerCatalog, verifyCenterAsset } from './center.ts';

const HEADERS = {
  'Cache-Control': 'no-store, max-age=0', 'CDN-Cache-Control': 'no-store',
  'Cloudflare-CDN-Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff',
  'Referrer-Policy': 'no-referrer', 'X-Robots-Tag': 'noindex',
};
function response(body: string, status: number, type = 'application/json; charset=utf-8', extra = {}) {
  return new Response(body, { status, headers: { ...HEADERS, 'Content-Type': type, ...extra } });
}
function unavailable(isAPI: boolean, code: string) {
  const extra = { 'Retry-After': '30', 'X-Serylane-Error': code };
  if (isAPI) return response(JSON.stringify({ error: 'LATEST_UNAVAILABLE', message: '下载中心暂时无法确认最新正式版，请稍后重试。' }), 503, undefined, extra);
  return response('<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>下载暂不可用 — Serylane</title><link rel="stylesheet" href="/styles.css"></head><body class="document-page"><main class="document-main"><h1>下载中心暂时不可用。</h1><p>尚未确认最新正式版安装包，请稍后刷新此页重试。本次没有开始下载。</p><p><a href="/#download">返回下载区重试</a></p></main></body></html>', 503, 'text/html; charset=utf-8', {
    ...extra, 'Content-Security-Policy': "default-src 'none'; style-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
  });
}

export async function handle(request: Request, env: Pick<Env, 'ASSETS'>, fetcher: Fetcher = fetch): Promise<Response> {
  const url = new URL(request.url);
  if (!url.pathname.startsWith('/api/') && !url.pathname.startsWith('/download/')) return env.ASSETS.fetch(request);
  const isAPI = url.pathname === '/api/releases/latest';
  const isResolve = url.pathname.startsWith('/api/download/');
  const target = url.pathname.slice(isResolve ? '/api/download/'.length : '/download/'.length) as Target;
  const isDownload = url.pathname.startsWith('/download/') && TARGETS.includes(target);
  let result: Response;
  if (!isAPI && !isDownload && !(isResolve && TARGETS.includes(target))) result = response('{"error":"NOT_FOUND"}', 404);
  else if (!['GET', 'HEAD'].includes(request.method)) result = response('{"error":"METHOD_NOT_ALLOWED"}', 405, undefined, { Allow: 'GET, HEAD' });
  // Former channel overrides are rejected before any outbound request.
  else if (url.search) result = response('{"error":"INVALID_QUERY"}', 400);
  else try {
    const catalog = await centerCatalog(fetcher);
    if (isAPI) result = response(JSON.stringify(catalog), 200, undefined, { 'X-Serylane-Resolver': 'download-center' });
    else {
      const asset = catalog.assets[target];
      await verifyCenterAsset(fetcher, asset);
      const headers = { 'X-Serylane-Version': catalog.version, 'X-Serylane-Channel': 'hk', 'X-Serylane-Resolver': 'download-center' };
      result = isResolve
        ? response(JSON.stringify({ version: catalog.version, channel: 'hk', url: asset.fileUrl }), 200, undefined, headers)
        : new Response(null, { status: 302, headers: { ...HEADERS, ...headers, Location: asset.fileUrl } });
    }
  } catch (error) {
    // Log only bounded, internal codes; never include URLs, credentials or response bodies.
    const code = error instanceof ReleaseError ? error.code : 'CENTER_UNAVAILABLE';
    console.warn({ event: 'release_resolution_failed', code });
    result = unavailable(isAPI || isResolve, code);
  }
  if (request.method === 'HEAD') { await result.body?.cancel(); return new Response(null, result); }
  return result;
}
export default { fetch: (request, env) => handle(request, env) } satisfies ExportedHandler<Env>;
