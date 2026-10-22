import { pathToFileURL } from 'node:url';
import { app } from 'electron';

/**
 * Being the default browser: the OS hands Flecha links (macOS `open-url`, other platforms
 * a second launch with the URL in argv) and .html files (`open-file`). Links can arrive
 * before the window exists, so they wait in a queue until `deliverLinksTo` is called.
 */
const pending: string[] = [];
let deliver: ((url: string) => void) | undefined;

function receive(url: string) {
  if (deliver) deliver(url);
  else pending.push(url);
}

function linksIn(argv: string[]) {
  return argv.filter((arg) => /^https?:\/\//i.test(arg));
}

/**
 * Call before `ready`: macOS sends `open-url` very early when Flecha is launched by a link.
 * `onSecondLaunch` runs when Flecha is launched again without a link.
 */
export function listenForExternalLinks(onSecondLaunch: () => void) {
  app.on('open-url', (event, url) => {
    event.preventDefault();
    receive(url);
  });
  app.on('open-file', (event, file) => {
    event.preventDefault();
    receive(pathToFileURL(file).href);
  });
  app.on('second-instance', (_event, argv) => {
    const links = linksIn(argv);
    // A launch with links leaves showing them to whoever receives them (a Flechinha may be in front).
    if (!links.length) return onSecondLaunch();
    links.forEach(receive);
  });
  linksIn(process.argv).forEach(receive);
}

export function deliverLinksTo(open: (url: string) => void) {
  deliver = open;
  pending.splice(0).forEach(open);
}

export function isDefaultBrowser() {
  return app.isDefaultProtocolClient('http') && app.isDefaultProtocolClient('https');
}

/**
 * Only a packaged Flecha.app can register: from `npm start` the OS would register the
 * generic Electron binary instead.
 */
export function canSetDefaultBrowser() {
  return app.isPackaged;
}

export function makeDefaultBrowser() {
  if (!canSetDefaultBrowser()) return false;
  // On macOS this shows the system's "use Flecha as your default browser?" prompt.
  return app.setAsDefaultProtocolClient('http') && app.setAsDefaultProtocolClient('https');
}
