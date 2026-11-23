import { createServer } from 'node:http';
import { writeFileSync } from 'node:fs';

const server = createServer((request, response) => {
  const path = new URL(request.url, 'http://localhost').pathname;
  if (path === '/favicon.ico') {
    response.writeHead(204).end();
    return;
  }
  const cookie = /(?:^|; )flecha_phase1=isolated(?:;|$)/.test(request.headers.cookie ?? '');
  if (path === '/cookie/set') {
    response.setHeader('Set-Cookie', 'flecha_phase1=isolated; Path=/; Max-Age=3600; HttpOnly; SameSite=Lax');
  }
  const name = path === '/cookie/check' ? (cookie ? 'Cookie present' : 'Cookie absent') : `Flecha fixture ${path}`;
  response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' });
  response.end(`<!doctype html>
    <title>${name}</title>
    <style>
      html { background: #147a70; color: white; font: 24px system-ui; }
      body { margin: 32px; }
      #marker { width: 116px; height: 56px; background: #ffcc49; }
    </style>
    <div id="marker"></div>
    <h1>${name}</h1>
    <input aria-label="Draft">
    <a href="/peek" target="_blank">Open another page</a>`);
});
server.listen(0, '127.0.0.1', () => writeFileSync(process.argv[2], String(server.address().port) + '\n'));
