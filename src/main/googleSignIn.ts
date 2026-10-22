import { app, type Session, type WebContents } from 'electron';

/**
 * Google refuses sign-in from browsers it can't identify ("This browser or app may not be
 * secure"), and a Chromium without Google's own brand in its client hints is one of those.
 * On Google's sign-in pages only, Flecha presents itself as Firefox, which Google accepts;
 * everywhere else, including the Google services you end up signed in to, it stays Chromium.
 */
const FIREFOX_USER_AGENT = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:140.0) Gecko/20100101 Firefox/140.0';

export function isGoogleSignIn(url: string) {
  try {
    return new URL(url).hostname === 'accounts.google.com';
  } catch {
    return false;
  }
}

/**
 * Requests to the sign-in pages carry Firefox's user agent and none of Chromium's client hints.
 * The page that comes after sign-in is requested before the tab switches back (below), so it
 * gets the normal user agent here.
 */
export function fixGoogleSignIn(ses: Session) {
  ses.webRequest.onBeforeSendHeaders({ urls: ['<all_urls>'] }, (details, callback) => {
    const headers = details.requestHeaders;
    if (isGoogleSignIn(details.url)) {
      for (const name of Object.keys(headers)) if (/^sec-ch-ua/i.test(name)) delete headers[name];
      headers['User-Agent'] = FIREFOX_USER_AGENT;
    } else if (details.resourceType === 'mainFrame' && headers['User-Agent'] === FIREFOX_USER_AGENT) {
      headers['User-Agent'] = app.userAgentFallback;
    }
    callback({ requestHeaders: headers });
  });
}

/** What scripts on the sign-in page see (navigator.userAgent) follows the page in and out. */
export function followGoogleSignIn(wc: WebContents) {
  const normal = wc.getUserAgent();
  wc.on('did-start-navigation', (details) => {
    if (!details.isMainFrame || details.isSameDocument) return;
    const want = isGoogleSignIn(details.url) ? FIREFOX_USER_AGENT : normal;
    if (wc.getUserAgent() !== want) wc.setUserAgent(want);
  });
}
