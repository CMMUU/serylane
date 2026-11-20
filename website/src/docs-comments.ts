import { boundedText } from './releases.ts';
import type { Fetcher } from './releases.ts';
import type { GithubUpdateCache } from './github-updates.ts';

export const DOCS_COMMENTS_PAGE = '/zh-hant/docs/';
export const DOCS_COMMENTS_API = '/api/docs/zh-hant/comments';
export const DOCS_COMMENTS_THREAD = 'https://github.com/CMMUU/serylane/issues/52';
const upstream = 'https://api.github.com/repos/CMMUU/serylane/issues/52/comments?per_page=6';
const key = 'https://serylane.cmmuu.com' + DOCS_COMMENTS_API;
const headers = { 'X-Content-Type-Options': 'nosniff', 'X-Robots-Tag': 'noindex', 'Cache-Control': 'no-store' };
const previewLimit = 420;

type CommentPreview = { id: number; author: string; text: string; createdAt: string };
const inFlight = new WeakMap<Fetcher, Promise<CommentPreview[]>>();

async function readPreviews(fetcher: Fetcher): Promise<CommentPreview[]> {
  const options: RequestInit = {
    method: 'GET',
    redirect: 'manual',
    signal: AbortSignal.timeout(5000),
    headers: { Accept: 'application/vnd.github.text+json', 'User-Agent': 'Serylane-Docs-Feedback' },
  };
  let response = await fetcher(upstream, options);
  if (response.ok) {
    // GitHub lists these oldest first. Read only the final small page when paginated.
    const lastLink = response.headers.get('link')?.match(/<([^<>]+)>;\s*rel="last"/)?.[1];
    const page = lastLink && new URL(lastLink).searchParams.get('page');
    if (page && /^[1-9]\d{0,3}$/.test(page) && Number(page) > 1) {
      await response.body?.cancel();
      response = await fetcher(upstream + '&page=' + page, options);
    }
  }
  const feed = JSON.parse(await boundedText(response, 1024 * 1024));
  if (!Array.isArray(feed)) throw new Error('Unrecognized GitHub comment feed');
  const result: CommentPreview[] = [];
  for (const item of feed.slice(0, 6)) {
    if (!item || !Number.isSafeInteger(item.id) || item.id <= 0 || typeof item.body_text !== 'string'
        || typeof item.created_at !== 'string' || !Number.isFinite(Date.parse(item.created_at))) {
      throw new Error('Unrecognized GitHub comment');
    }
    const login = item.user?.login;
    const author = typeof login === 'string' && /^[a-z\d][a-z\d-]{0,38}(?:\[bot\])?$/i.test(login) ? login : 'ghost';
    const text = [...item.body_text.trim()];
    if (!text.length) continue;
    result.push({
      id: item.id,
      author,
      text: text.length > previewLimit ? text.slice(0, previewLimit).join('') + '…' : text.join(''),
      createdAt: new Date(item.created_at).toISOString(),
    });
  }
  return result;
}

function sharedPreviews(fetcher: Fetcher) {
  const pending = inFlight.get(fetcher);
  if (pending) return pending;
  const task = readPreviews(fetcher).finally(() => inFlight.delete(fetcher));
  inFlight.set(fetcher, task);
  return task;
}

export async function docsComments(request: Request, url: URL, fetcher: Fetcher, edge?: GithubUpdateCache): Promise<Response> {
  const error = (status: number, reason: string, additional = {}) => Response.json({ error: reason }, {
    status, headers: { ...headers, ...additional },
  });
  let result: Response;
  if (request.method !== 'GET' && request.method !== 'HEAD') result = error(405, 'METHOD_NOT_ALLOWED', { Allow: 'GET, HEAD' });
  else if (url.search) result = error(400, 'INVALID_QUERY');
  else {
    const cached = await edge?.cache.match(key).catch(() => undefined);
    if (cached) result = cached;
    else try {
      result = Response.json({ comments: await sharedPreviews(fetcher) }, {
        headers: { ...headers, 'Cache-Control': 'public, max-age=300' },
      });
      if (edge) edge.waitUntil(edge.cache.put(key, result.clone()).catch(() => {}));
    } catch {
      result = error(503, 'COMMENTS_UNAVAILABLE', { 'Retry-After': '30' });
    }
  }
  if (request.method === 'HEAD') {
    void result.body?.cancel().catch(() => {});
    return new Response(null, result);
  }
  return result;
}
