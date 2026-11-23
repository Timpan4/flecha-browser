// Loopback HTTP fixtures for the Xvfb action scripts, so no check depends on the network.
//
//   node fixture-server.mjs <port-file>
//
// Listens on 127.0.0.1 on a port the OS picks and writes the port to <port-file>.
// /marker   logs FLECHA_CONSOLE_MARKER to the console and sets a persistent cookie.
// /page     a plain page, for navigation checks.
import { writeFileSync } from 'node:fs';
import { createServer } from 'node:http';

const portFile = process.argv[2];

if (!portFile) {
  console.error('usage: node fixture-server.mjs <port-file>');
  process.exit(2);
}

const pages = {
  '/marker': {
    // Any lifetime longer than the test run keeps the cookie on disk across a restart.
    headers: { 'Set-Cookie': 'flecha_fixture=1; Max-Age=86400; Path=/' },
    body: "<!doctype html><title>marker</title><script>console.log('FLECHA_CONSOLE_MARKER');</script>",
  },
  '/page': {
    headers: {},
    body: '<!doctype html><title>fixture page</title><p>fixture page</p>',
  },
};

const server = createServer((request, response) => {
  const page = pages[new URL(request.url, 'http://127.0.0.1').pathname];

  if (!page) {
    response.writeHead(404).end();

    return;
  }

  response.writeHead(200, {
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
