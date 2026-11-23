// Loopback HTTP fixtures for the Xvfb action scripts, so no check depends on the network.
//
//   node fixture-server.mjs <port-file>
//
// Listens on 127.0.0.1 on a port the OS picks and writes the port to <port-file>. Prints one
// line per request, `<method> <path and query>`, which the action scripts wait for.
// /marker   logs two FLECHA_CONSOLE_MARKER lines naming the case (its query), one plain and
//           one with a spoofed file:// source, then requests /ran with its own query, and
//           sets a persistent cookie. A `GET /ran` line proves the page loaded
//           and its script ran.
// /ran      empty response.
// /page     a plain page, for navigation checks.
import { writeFileSync } from 'node:fs';
import { createServer } from 'node:http';

const portFile = process.argv[2];

if (!portFile) {
  console.error('usage: node fixture-server.mjs <port-file>');
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

const pages = {
  '/marker': {
    // Any lifetime longer than the test run keeps the cookie on disk across a restart.
    headers: { 'Set-Cookie': 'flecha_fixture=1; Max-Age=86400; Path=/' },
    body: marker,
  },
  '/ran': { status: 204, headers: {}, body: '' },
  '/page': {
    headers: {},
    body: '<!doctype html><title>fixture page</title><p>fixture page</p>',
  },
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

server.listen(0, '127.0.0.1', () => {
  writeFileSync(portFile, String(server.address().port));
});

for (const signal of ['SIGTERM', 'SIGINT']) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
