import { createServer } from 'node:http';
import { once } from 'node:events';

export async function fixture() {
  const pending = new Map();

  const server = createServer((request, response) => {
  const path = new URL(request.url, 'http://localhost').pathname;

  if (path.startsWith('/probe/')) {
    const name = decodeURIComponent(path.slice('/probe/'.length));
    const value = new URL(request.url, 'http://localhost').searchParams.get('value');
    pending.get(name)?.(value);
    pending.delete(name);
    response.writeHead(204).end();

    return;
  }

  if (path === '/favicon.ico') {
    response.writeHead(204).end();

    return;
  }

  const cookie = /(?:^|; )flecha_phase1=isolated(?:;|$)/.test(request.headers.cookie ?? '');

  if (path === '/cookie/set') {
    response.setHeader('Set-Cookie', 'flecha_phase1=isolated; Path=/; HttpOnly; SameSite=Lax');
  }

  const name = path === '/cookie/check' ? (cookie ? 'Cookie present' : 'Cookie absent') : `Flecha fixture ${path}`;

  const [background, marker] = {
    '/split': ['#703496', '#f386b9'],
    '/peek': ['#235089', '#83ddff'],
  }[path] ?? ['#147a70', '#ffcc49'];

  response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' });
  response.end(`<!doctype html>
    <title>${name}</title>
    <style>
      html { background: ${background}; color: white; font: 24px system-ui; }
      body { margin: 32px; }
      #marker { width: 116px; height: 56px; background: ${marker}; }
    </style>
    <div id="marker"></div>
    <h1>${name}</h1>
    <input aria-label="Draft">
    <a href="/peek" target="_blank">Open another page</a>
    <script>window.documentToken = crypto.randomUUID();</script>`);
  });

  server.listen(0, '127.0.0.1');
  await once(server, 'listening');

  return {
    server,
    base: `http://127.0.0.1:${server.address().port}`,
    probe: name => new Promise(resolve => pending.set(name, resolve)),
  };
}
