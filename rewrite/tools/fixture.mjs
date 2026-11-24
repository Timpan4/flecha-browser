import { createServer } from 'node:http';
import { once } from 'node:events';

export const blankRestartColor = '#E64DA3';

// Popup fixture colours, unique on screen so a capture can find each page's marker.
export const openerColors = ['#2B2D6E', '#F2E94E'];
export const popupColors = ['#5A1F1F', '#9AF2C4'];

// The /select page's colours, and the colour of its <select> popup's options.
export const selectColors = ['#1F4D2E', '#C7F25A'];
export const selectOptionColor = '#3B1FD1';

// The bytes of /jar/download.
export const downloadBody = 'Flecha fixture download\n';

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

// Titles a jar page once its storage step has run: the jar, what the cookie check found, and
// whether the page reads its localStorage entry back. A write that never happened reads `empty`.
const jarReport = (jar, cookie, step) => `<script>
  ${step}
  const stored = localStorage.getItem('${jar}') === '${jar}_stored';
  document.title = '${jar}:${cookie}:' + (stored ? 'stored' : 'empty');
</script>`;

export async function fixture(handle = () => false) {
  const pending = new Map();
  const requests = [];

  const server = createServer((request, response) => {
  const path = new URL(request.url, 'http://localhost').pathname;

  if (handle(path, response, request)) return;

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

  // A cookie and a localStorage entry named by the caller, so a scan of the profile on disk
  // can look for that name. Max-Age makes the cookie one a persistent jar writes to disk.
  const jar = /^[a-z0-9_]+$/.exec(new URL(request.url, 'http://localhost').searchParams.get('jar') ?? '')?.[0];
  const clipboardFrame = new URL(request.url, 'http://localhost').searchParams.has('frame');
  const inJar = jar !== undefined && (request.headers.cookie ?? '').split('; ').includes(`${jar}=1`);

  if (jar && path === '/jar/set') {
    response.setHeader('Set-Cookie', `${jar}=1; Path=/; Max-Age=86400; SameSite=Lax`);
  }

  // A file named after the jar. Its bytes never name the jar, so a scan of the profile finds a
  // download record that names it but not the saved file.
  if (jar && path === '/jar/download') {
    response.writeHead(200, { 'Content-Type': 'text/plain', 'Content-Disposition': `attachment; filename="${jar}.txt"` });
    response.end(downloadBody);

    return;
  }

  const cookieResult = { isolated: 'Cookie present', shared: 'Shared cookie' };

  const jarTitle = path === '/jar/list'
    ? `jars:${(request.headers.cookie ?? '').split('; ').map(entry => entry.split('=')[0]).filter(name => /^flecha_[a-z0-9_]+$/.test(name)).sort().join(',')}`
    : undefined;

  const name = jarTitle || (path === '/cookie/check' ? (cookieResult[cookie] ?? 'Cookie absent') : `Flecha fixture ${path}`);

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
    // Editable fields for the clipboard checks. The page counts Chromium's own paste events.
    '/clipboard': `
      <input id="text" aria-label="Text">
      <input id="password" type="password" aria-label="Password">
      <textarea id="area" aria-label="Notes"></textarea>
      <textarea id="area2" aria-label="More notes"></textarea>
      <div id="editable" contenteditable="true">x</div>
      <input id="email" type="email" aria-label="Email">
      <input id="check" type="checkbox" aria-label="Remember me">
      <div id="host"></div>
      ${clipboardFrame ? '' : `<iframe id="frame" src="http://localhost:${request.socket.localPort}/clipboard?frame=1"></iframe>`}
      <script>
        // A password the selection read cannot see into.
        const shadow = document.querySelector('#host').attachShadow({ mode: 'closed' });
        shadow.innerHTML = '<input type="password" aria-label="Hidden password">';
        window.shadowPassword = shadow.querySelector('input');
        // The cross-site frame edits its fields when its parent asks, and reports its state
        // under the probe label the parent sends.
        const fields = {
          text: document.querySelector('#text'),
          password: document.querySelector('#password'),
          area: document.querySelector('#area'),
        };
        addEventListener('message', event => {
          if (event.source !== parent || !Array.isArray(event.data)) return;
          const [command, label] = event.data;
          if (command === 'focus') fields.text.focus();
          if (command === 'clear-area') {
            fields.area.value = '';
            fields.area.focus();
          }
          if (command === 'select' || command === 'select-password') {
            const field = command === 'select' ? fields.text : fields.password;
            field.value = command === 'select' ? 'flecha-frame-copy' : 'flecha-frame-secret';
            field.focus();
            field.select();
          }
          if (command !== 'report') return;
          const active = document.activeElement;
          const box = fields.area.getBoundingClientRect();
          const state = {
            active: active?.id ?? null,
            selection: [active?.selectionStart ?? null, active?.selectionEnd ?? null],
            text: fields.text.value,
            password: fields.password.value,
            area: fields.area.value,
            areaCenter: [box.x + box.width / 2, box.y + box.height / 2],
            editKeys: window.editKeys,
          };
          fetch('/probe/' + label + '?value=' + encodeURIComponent(JSON.stringify(state)));
        });
        window.pastes = 0;
        addEventListener('paste', () => window.pastes++, true);
        // Ctrl+C and Ctrl+X that reached the page as keystrokes. Chromium's own copy would make
        // it the X11 clipboard owner, and GPUI's next read would then wait on it.
        window.editKeys = 0;
        addEventListener('keydown', event => {
          if (!event.ctrlKey || !['c', 'x'].includes(event.key)) return;
          window.editKeys++;
          event.preventDefault();
        }, true);
      </script>`,
    '/fullscreen': `
      <button id="full">Fullscreen</button>
      <script>document.querySelector('#full').addEventListener('click', () => document.documentElement.requestFullscreen());</script>`,
    '/select': `
      <style>
        #choice { position: absolute; left: 400px; top: 32px; width: 240px; height: 48px; font: 24px system-ui; }
        option { background: ${selectOptionColor}; color: white; }
      </style>
      <select id="choice"><option>One</option><option>Two</option><option>Three</option></select>`,
    '/jar/set': jar ? jarReport(jar, 'set', `localStorage.setItem('${jar}', '${jar}_stored');`) : '',
    '/jar/check': jar ? jarReport(jar, inJar ? 'present' : 'absent', '') : '',
  }[path] ?? '';

  response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' });
  response.end(`<!doctype html>
    <title>${name}</title>
    <style>
      html { background: ${background}; color: white; font: 24px system-ui; }
      body { margin: 32px; }
      #marker { width: 116px; height: 56px; background: ${marker}; }
      #blank, #scripted, #full { display: block; width: 320px; height: 64px; margin: 8px 0; font: inherit; }
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
    // A probe the page never sends rejects after `timeoutMs`, when given.
    probe: (name, timeoutMs) => new Promise((resolve, reject) => {
      pending.set(name, resolve);

      if (timeoutMs === undefined) return;
      setTimeout(() => {
        if (pending.get(name) !== resolve) return;
        pending.delete(name);
        reject(new Error(`no ${name} probe within ${timeoutMs} ms`));
      }, timeoutMs).unref();
    }),
    requests,
  };
}
