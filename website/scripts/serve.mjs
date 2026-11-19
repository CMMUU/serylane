import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { resolve, sep, extname } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../.site/', import.meta.url));
const port = Number(process.env.SERYLANE_PREVIEW_PORT || 4177);
const types = { '.html': 'text/html; charset=utf-8', '.css': 'text/css; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.json': 'application/json; charset=utf-8', '.png': 'image/png', '.ico': 'image/x-icon', '.txt': 'text/plain; charset=utf-8', '.xml': 'application/xml; charset=utf-8' };
const isFile = async path => (await stat(path).catch(() => null))?.isFile();
const headers = { 'X-Content-Type-Options': 'nosniff', 'Referrer-Policy': 'strict-origin-when-cross-origin', 'X-Frame-Options': 'DENY', 'Cache-Control': 'no-store' };
const server = createServer(async (req, res) => {
  try {
    if (!['GET', 'HEAD'].includes(req.method)) { res.writeHead(405, { ...headers, Allow: 'GET, HEAD' }).end(); return; }
    const url = new URL(req.url, `http://127.0.0.1:${port}`);
    let pathname;
    try { pathname = decodeURIComponent(url.pathname); } catch { res.writeHead(400, headers).end(); return; }
    if (pathname.includes('\0') || pathname.includes('\\') || pathname.split('/').some(part => part.startsWith('.'))) {
      res.writeHead(400, headers).end(); return;
    }
    let path = resolve(root, `.${pathname}`);
    if (path !== resolve(root) && !path.startsWith(resolve(root) + sep)) { res.writeHead(403, headers).end(); return; }
    if (pathname.endsWith('/index.html')) {
      res.writeHead(308, { ...headers, Location: pathname.slice(0, -10) + url.search }).end(); return;
    }
    if (pathname.endsWith('.html') && pathname !== '/404.html' && await isFile(path)) {
      res.writeHead(308, { ...headers, Location: pathname.slice(0, -5) + url.search }).end(); return;
    }
    if ((await stat(path).catch(() => null))?.isDirectory()) {
      if (!pathname.endsWith('/')) { res.writeHead(308, { ...headers, Location: pathname + '/' + url.search }).end(); return; }
      path = resolve(path, 'index.html');
    } else if (!extname(pathname) && await isFile(path + '.html')) path += '.html';
    let status = pathname === '/404.html' ? 404 : 200;
    if (!await isFile(path) || ['_headers', '_redirects'].includes(pathname.split('/').at(-1))) {
      path = resolve(root, '404.html'); status = 404;
    }
    const data = await readFile(path);
    res.writeHead(status, { ...headers, 'Content-Type': types[extname(path)] || 'application/octet-stream', 'Content-Length': data.length });
    res.end(req.method === 'HEAD' ? undefined : data);
  } catch { res.writeHead(500, headers).end('Preview server error'); }
});
server.listen(port, '127.0.0.1', () => console.log(`Serylane preview: http://127.0.0.1:${port}`));
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => server.close(() => process.exit(0)));
