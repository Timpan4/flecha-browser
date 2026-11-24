import { createServer } from 'node:http';
import { once } from 'node:events';

export const blankRestartColor = '#E64DA3';

// Popup fixture colours, unique on screen so a capture can find each page's marker.
export const openerColors = ['#2B2D6E', '#F2E94E'];
export const popupColors = ['#5A1F1F', '#9AF2C4'];

// The /select page's colours, and the colour of its <select> popup's options.
export const selectColors = ['#1F4D2E', '#C7F25A'];
export const selectOptionColor = '#3B1FD1';

// The opener page's controls. Each sets a fresh title once window.open returns, so the
// title change reaches the shell after any popup event the call queued.
const openerScript = `
  let opened = 0;
  document.querySelector('#scripted').addEventListener('click', () => {
    const popup = window.open('/popup', 'flecha-popup', 'popup,width=480,height=360');
    document.title = 'after-popup:' + (++opened) + ':' + (popup !== null);
  });
  addEventListener('message', event => fetch('/probe/popup-message?value=' + encodeURIComponent(event.data)));
  if (new URLSearchParams(location.search).has('auto')) {
    addEventListener('load', () => {
      const popup = window.open('/blocked-popup', 'flecha-blocked', 'popup,width=480,height=360');
      const tab = window.open('/blocked-tab');
      document.title = 'after-blocked:' + (popup === null) + ':' + (tab === null);
    });
  }`;

export async function fixture(handle = () => false) {
  const pending = new Map();
  const requests = [];

  const server = createServer((request, response) => {
  const path = new URL(request.url, 'http://localhost').pathname;

  if (handle(path, response)) return;

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

  const cookie = /(?:^|; )flecha_phase1=(isolated|shared)(?:;|$)/.exec(request.headers.cookie ?? '')?.[1];

  requests.push({ path, search: new URL(request.url, 'http://localhost').search, referer: request.headers.referer, cookie });

  const write = { '/cookie/set': 'isolated', '/cookie/set-shared': 'shared' }[path];

  if (write) {
    response.setHeader('Set-Cookie', `flecha_phase1=${write}; Path=/; HttpOnly; SameSite=Lax`);
  }

  const cookieResult = { isolated: 'Cookie present', shared: 'Shared cookie' };
  const name = path === '/cookie/check' ? (cookieResult[cookie] ?? 'Cookie absent') : `Flecha fixture ${path}`;

  const [background, marker] = {
    '/split': ['#703496', '#f386b9'],
    '/peek': ['#235089', '#83ddff'],
    '/opener': openerColors,
    '/popup': popupColors,
    '/select': selectColors,
  }[path] ?? ['#147a70', '#ffcc49'];

  // Asks before leaving. Chromium shows the prompt only after a user gesture.
  const beforeUnload = `
    addEventListener('beforeunload', event => {
      event.preventDefault();
      event.returnValue = '';
    });`;

  const extra = {
    '/opener': `
      <a id="blank" href="/cookie/check" target="_blank">Open in a new tab</a>
      <button id="scripted">Open a popup</button>
      <script>${openerScript}</script>`,
    '/popup': `
      <script>
        fetch('/probe/popup-opener?value=' + (window.opener !== null));
        window.opener?.postMessage('hello-from-popup', '*');${beforeUnload}
      </script>`,
    '/unload': `<script>${beforeUnload}</script>`,
    '/select': `
      <style>
        #choice { position: absolute; left: 400px; top: 32px; width: 240px; height: 48px; font: 24px system-ui; }
        option { background: ${selectOptionColor}; color: white; }
      </style>
      <select id="choice"><option>One</option><option>Two</option><option>Three</option></select>`,
  }[path] ?? '';

  response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' });
  response.end(`<!doctype html>
    <title>${name}</title>
    <style>
      html { background: ${background}; color: white; font: 24px system-ui; }
      body { margin: 32px; }
      #marker { width: 116px; height: 56px; background: ${marker}; }
      #blank, #scripted { display: block; width: 320px; height: 64px; margin: 8px 0; font: inherit; }
    </style>
    <div id="marker"></div>
    <h1>${name}</h1>
    <input aria-label="Draft">
    <a href="/peek" target="_blank">Open another page</a>
    <script>window.documentToken = crypto.randomUUID();</script>${extra}`);
  });

  server.listen(0, '127.0.0.1');
  await once(server, 'listening');

  return {
    server,
    base: `http://127.0.0.1:${server.address().port}`,
    probe: name => new Promise(resolve => pending.set(name, resolve)),
    requests,
  };
}
