import { GITHUB, TARGETS, catalogAsset, latestCatalog, mirrorReady, resolveDownload, ReleaseError } from './releases.ts';
import type { Fetcher, Target } from './releases.ts';

const HEADERS = {
  'Cache-Control': 'no-store, max-age=0', 'CDN-Cache-Control': 'no-store',
  'Cloudflare-CDN-Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff',
  'Referrer-Policy': 'no-referrer', 'X-Robots-Tag': 'noindex',
};
function response(body: string, status: number, type = 'application/json; charset=utf-8', extra = {}) {
  return new Response(body, { status, headers: { ...HEADERS, 'Content-Type': type, ...extra } });
}
function unavailable(isAPI: boolean, code: string) {
  if (isAPI) return response(JSON.stringify({ error: 'LATEST_UNAVAILABLE', message: '暂时无法确认最新正式版，请稍后重试。' }), 503, undefined, { 'Retry-After': '30', 'X-Serylane-Error': code });
  return response(`<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>下载暂不可用 — Serylane</title><link rel="stylesheet" href="/styles.css"></head><body class="document-page"><main class="document-main"><h1>暂时无法确认最新安装包。</h1><p>发布源或下载渠道暂时不可用，本次没有转向旧版本。请稍后刷新此页重试。</p><p><a href="/#download">返回下载区</a> · <a href="${GITHUB}/releases" rel="noreferrer">手动查看 GitHub 正式发布</a></p><p>手动下载前请自行核对版本、系统与架构。</p></main></body></html>`, 503, 'text/html; charset=utf-8', {
    'Retry-After': '30', 'X-Serylane-Error': code, 'Content-Security-Policy': "default-src 'none'; style-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
  });
}

export async function handle(request: Request, env: Pick<Env, 'ASSETS'>, fetcher: Fetcher = fetch): Promise<Response> {
  const url = new URL(request.url);
  if (!url.pathname.startsWith('/api/') && !url.pathname.startsWith('/download/')) return env.ASSETS.fetch(request);
  const isAPI = url.pathname === '/api/releases/latest';
  const target = url.pathname.slice('/download/'.length) as Target;
  const isDownload = url.pathname.startsWith('/download/') && TARGETS.includes(target);
  if (!isAPI && !isDownload) return response('{"error":"NOT_FOUND"}', 404);
  if (!['GET', 'HEAD'].includes(request.method)) return response('{"error":"METHOD_NOT_ALLOWED"}', 405, undefined, { Allow: 'GET, HEAD' });
  // Only a literal GitHub override is supported; no arbitrary upstream or version parameters.
  if (url.search && (isAPI || url.search !== '?channel=github')) return response('{"error":"INVALID_QUERY"}', 400);
  let result: Response;
  try {
    if (isAPI) {
      const catalog = await latestCatalog(fetcher);
      const [hk, mirror] = await Promise.all([
        mirrorReady(fetcher, catalog, TARGETS, 'hk'), mirrorReady(fetcher, catalog, TARGETS, 'gitee'),
      ]);
      result = response(JSON.stringify({
        version: catalog.version, checkedAt: new Date().toISOString(),
        assets: Object.fromEntries(TARGETS.map(target => [target, { ...catalogAsset(catalog, target), domesticAvailable: hk.ready.has(target) || mirror.ready.has(target), hkAvailable: hk.ready.has(target), giteeAvailable: mirror.ready.has(target) }])),
      }), 200, undefined, { 'X-Serylane-Mirror': mirror.diagnostic, 'X-Serylane-HK': hk.diagnostic });
    } else {
      const download = await resolveDownload(fetcher, target, url.search === '?channel=github');
      result = new Response(null, { status: 302, headers: { ...HEADERS, Location: download.url,
        'X-Serylane-Version': download.version, 'X-Serylane-Channel': download.channel } });
    }
  } catch (error) {
    // Do not log request URLs, upstream signed URLs, visitor data or arbitrary exception text.
    console.warn({ event: 'release_resolution_failed', code: error instanceof ReleaseError ? error.code : 'UPSTREAM_UNAVAILABLE' });
    result = unavailable(isAPI, error instanceof ReleaseError ? error.code : 'UPSTREAM_UNAVAILABLE');
  }
  if (request.method === 'HEAD') { await result.body?.cancel(); return new Response(null, result); }
  return result;
}

export default { fetch: (request, env) => handle(request, env) } satisfies ExportedHandler<Env>;
