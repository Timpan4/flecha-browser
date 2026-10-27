/** Shared by extension pages and the service worker, whose navigator also exposes platform. */
export const isMac = /Mac/i.test(globalThis.navigator?.platform ?? '');

export const primaryModifier = (event: { metaKey: boolean; ctrlKey: boolean }) => (isMac ? event.metaKey : event.ctrlKey);
