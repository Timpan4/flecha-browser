/** What Flecha pages and the shell's background say to each other over the "flecha" port. */
export type Message =
  | { type: 'cmd'; name: string; args: unknown[] }
  | { type: 'invoke'; id: number; name: string; args: unknown[] }
  | { type: 'result'; id: number; value?: unknown; error?: string }
  | { type: 'event'; name: string; args: unknown[] }
  /** Keeps the background awake while a page is open (it sleeps after 30s of silence). */
  | { type: 'ping' };
