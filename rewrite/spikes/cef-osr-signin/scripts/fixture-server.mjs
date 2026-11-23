// Loopback HTTP fixtures for the Xvfb action scripts, so no check depends on the network.
//
//   node fixture-server.mjs <port-file> [host]
//
// Listens on <host> (default 127.0.0.1) on a port the OS picks and writes the port to
// <port-file>. Prints one line per request, `<method> <path and query>`, which the action
// scripts wait for. A second server on 127.0.0.2 stands in for a remote origin: it is on the
// loopback interface, so nothing leaves the host, but the spike's --fake-camera rule treats
// only 127.0.0.1, localhost and [::1] as local.
// /marker   logs two FLECHA_CONSOLE_MARKER lines naming the case (its query), one plain and
//           one with a spoofed file:// source, then requests /ran with its own query, and
//           sets a persistent cookie. A `GET /ran` line proves the page loaded
//           and its script ran.
// /ran      empty response.
// /page     a plain page, for navigation checks.
// /guard    ?remote=<base URL>: loads <base>/frame in an iframe, <base>/img as an image,
//           fetches <base>/fetch, and fetches /to?<base>/redirected; when the image and the
//           fetches have settled it requests /done.
// /done     empty response.
// /to       302 to the URL in its query.
import { writeFileSync } from 'node:fs';
import { createServer } from 'node:http';

const portFile = process.argv[2];
const host = process.argv[3] ?? '127.0.0.1';

if (!portFile) {
  console.error('usage: node fixture-server.mjs <port-file> [host]');
  process.exit(2);
}

// Two lines per load, both naming the case (the query): a plain one, and one whose console
// source is spoofed to a file:// URL with `//# sourceURL`.
const marker =
  '<!doctype html><title>marker</title><script>' +
  "const where = location.search.slice(1) || 'none';" +
  "console.log('FLECHA_CONSOLE_MARKER plain ' + where);" +
  'eval("console.log(\'FLECHA_CONSOLE_MARKER spoof \' + where)\\n//# sourceURL=file:///flecha-spoof.js");' +
  "fetch('/ran' + location.search);" +
  '</script>';

// Every way a page can reach another origin without a click: a frame, a subresource, a
// fetch, and a fetch that this server redirects there. When the image and both fetches have
// settled (load, error or a rejected fetch) the page requests /done from its own server. The
// frame is not waited for: a frame whose navigation is cancelled never fires load or error.
const guard =
  '<!doctype html><title>guard</title><body><script>' +
  "const remote = new URLSearchParams(location.search).get('remote');" +
  'const settle = p => p.then(() => 0, () => 0);' +
  'const element = (tag, src) => new Promise(done => {' +
  '  const e = document.createElement(tag); e.onload = e.onerror = done; e.src = src;' +
  '  document.body.append(e); });' +
  "element('iframe', remote + '/frame');" +
  'Promise.all([' +
  "  element('img', remote + '/img')," +
  "  settle(fetch(remote + '/fetch', { mode: 'no-cors' }))," +
  "  settle(fetch('/to?' + remote + '/redirected', { mode: 'no-cors' }))," +
  "]).then(() => fetch('/done'));" +
  '</script>';

const pages = {
  '/marker': {
    // Any lifetime longer than the test run keeps the cookie on disk across a restart.
    headers: { 'Set-Cookie': 'flecha_fixture=1; Max-Age=86400; Path=/' },
    body: marker,
  },
  '/ran': { status: 204, headers: {}, body: '' },
  '/done': { status: 204, headers: {}, body: '' },
  '/page': {
    headers: {},
    body: '<!doctype html><title>fixture page</title><p>fixture page</p>',
  },
  '/guard': { headers: {}, body: guard },
};

const server = createServer((request, response) => {
  let url;

  try {
    url = new URL(request.url, 'http://127.0.0.1');
  } catch {
    console.log(`${request.method} <invalid target>`);
    response.writeHead(400).end();

    return;
  }

  console.log(`${request.method} ${url.pathname}${url.search}`);

  if (url.pathname === '/to') {
    response
      .writeHead(302, { Location: decodeURIComponent(url.search.slice(1)) })
      .end();

    return;
  }

  const page = pages[url.pathname];

  if (!page) {
    response.writeHead(404).end();

    return;
  }

  response.writeHead(page.status ?? 200, {
    'Content-Type': 'text/html; charset=utf-8',
    ...page.headers,
  });
  response.end(page.body);
});

server.listen(0, host, () => {
  writeFileSync(portFile, String(server.address().port));
});

for (const signal of ['SIGTERM', 'SIGINT']) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
