const SEARCH_URL = 'https://www.google.com/search?q=';

/** Turns whatever was typed in the command bar into a URL to load. */
export function resolveInput(raw: string): string {
  const input = raw.trim();
  if (!input) return 'about:blank';
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(input) || /^(about|data|file):/i.test(input)) return input;
  if (/^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?(\/.*)?$/i.test(input)) return `http://${input}`;
  if (!/\s/.test(input) && /^[^/]+\.[a-z]{2,}(:\d+)?(\/.*)?$/i.test(input)) return `https://${input}`;
  return SEARCH_URL + encodeURIComponent(input);
}
