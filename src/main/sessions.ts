import { session, type Session } from 'electron';
import type { Space } from '../shared/types';

/**
 * Tabs normally share the default session. A Space with "separate cookies and logins"
 * gets its own persistent partition, so you can be signed in as different users side by side.
 * Every session tabs use gets the same setup (ad blocking, downloads, permission prompts).
 */
const configured = new Set<Session>();
const setups: Array<(ses: Session) => void> = [];

export function onEachSession(setup: (ses: Session) => void) {
  setups.push(setup);
  for (const ses of configured) setup(ses);
}

export function sessionForSpace(space: Space | undefined): Session {
  const ses = space?.isolated ? session.fromPartition(`persist:space-${space.id}`) : session.defaultSession;
  if (!configured.has(ses)) {
    configured.add(ses);
    for (const setup of setups) setup(ses);
  }
  return ses;
}
