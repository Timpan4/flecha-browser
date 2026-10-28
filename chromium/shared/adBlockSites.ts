/**
 * Sites the ad blocker leaves alone, as uBlock Origin's trusted sites: an entry is a hostname
 * and covers its subdomains too, the way declarativeNetRequest's requestDomains match
 * (features/adblock.ts turns the list into one allowAllRequests rule).
 */

/**
 * A hostname requestDomains takes: lowercase ASCII labels, as URL.hostname writes them
 * (punycode for the rest). An IPv6 host ("[::1]") isn't one.
 */
export function isSiteHost(host: string) {
  return /^[a-z0-9-]+(\.[a-z0-9-]+)*$/.test(host);
}

/** Whether `site` is `host` or one of its parent domains. */
function covers(site: string, host: string) {
  return host === site || host.endsWith(`.${site}`);
}

/** Whether ads and trackers are blocked on `host`, given the sites where blocking is off. */
export function blocksAdsOn(allowed: readonly string[] | undefined, host: string) {
  return !allowed?.some((site) => covers(site, host));
}

/**
 * The list with blocking on or off for `host`. Turning it back on removes every entry that
 * covers `host`, so the switch always does what it shows.
 */
export function withAdBlocking(allowed: readonly string[] | undefined, host: string, block: boolean): string[] {
  const list = allowed ?? [];
  if (block) return list.filter((site) => !covers(site, host));
  return blocksAdsOn(list, host) ? [...list, host] : [...list];
}
