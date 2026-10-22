import { app, type Session, type WebContents } from 'electron';

/**
 * Google refuses sign-in from browsers it can't identify ("This browser or app may not be
 * secure"). Electron's Chromium reports itself as plain "Chromium" in its client hints, with
 * no browser brand, which is one of the things Google looks at. On Google's sign-in pages
 * Flecha presents itself exactly as Google Chrome of the same version: the same user agent,
 * and "Google Chrome" in the Sec-CH-UA headers and in navigator.userAgentData.
 */
const CHROME_MAJOR = process.versions.chrome.split('.')[0];
/** Chrome's reduced user agent, built on first use: never mentioning Electron or Flecha. */
const chromeUserAgent = () =>
  app.userAgentFallback
    .replace(/ (Electron|Flecha)\/\S+/gi, '')
    .replace(/Chrome\/[\d.]+/, `Chrome/${CHROME_MAJOR}.0.0.0`);
const BRANDS = [
  { brand: 'Google Chrome', version: CHROME_MAJOR },
  { brand: 'Chromium', version: CHROME_MAJOR },
  { brand: 'Not-A.Brand', version: '24' },
];
const SEC_CH_UA = BRANDS.map((b) => `"${b.brand}";v="${b.version}"`).join(', ');

export function isGoogleSignIn(url: string) {
  try {
    return new URL(url).hostname === 'accounts.google.com';
  } catch {
    return false;
  }
}

/** Requests to the sign-in pages carry Chrome's brand in their client hints. */
export function fixGoogleSignIn(ses: Session) {
  ses.webRequest.onBeforeSendHeaders({ urls: ['https://accounts.google.com/*'] }, (details, callback) => {
    const headers = details.requestHeaders;
    headers['User-Agent'] = chromeUserAgent();
    headers['sec-ch-ua'] = SEC_CH_UA;
    headers['sec-ch-ua-mobile'] = '?0';
    headers['sec-ch-ua-platform'] = '"macOS"';
    callback({ requestHeaders: headers });
  });
}

/**
 * What scripts on the sign-in page see. Chrome's identity is set through the DevTools
 * protocol (the only way to change navigator.userAgentData) as the tab heads to Google's
 * sign-in, and kept for the tab afterwards: it is the same Chromium either way.
 */
export function followGoogleSignIn(wc: WebContents) {
  let applied = false;
  const apply = () => {
    // Phone-sized mirrors keep their mobile identity.
    if (applied || /Mobile/.test(wc.getUserAgent())) return false;
    applied = true;
    try {
      if (!wc.debugger.isAttached()) wc.debugger.attach('1.3');
    } catch {
      // DevTools is open and holds the target; the command still goes through.
    }
    wc.debugger
      .sendCommand('Emulation.setUserAgentOverride', {
        userAgent: chromeUserAgent(),
        platform: 'MacIntel',
        userAgentMetadata: {
          brands: BRANDS,
          fullVersionList: BRANDS.map((b) => ({ ...b, version: b.brand === 'Not-A.Brand' ? '24.0.0.0' : process.versions.chrome })),
          platform: 'macOS',
          platformVersion: '15.0.0',
          architecture: process.arch === 'arm64' ? 'arm' : 'x86',
          bitness: '64',
          model: '',
          mobile: false,
          wow64: false,
        },
      })
      .catch(() => (applied = false));
    return true;
  };
  wc.on('did-start-navigation', (details) => {
    if (details.isMainFrame && isGoogleSignIn(details.url)) apply();
  });
  // A sign-in popup can start loading before this runs; its page then reloads with Chrome's identity.
  wc.once('dom-ready', () => {
    if (isGoogleSignIn(wc.getURL()) && apply()) wc.reload();
  });
}
