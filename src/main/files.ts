import fs from 'node:fs';
import path from 'node:path';
import type { StatePersistence } from './store';
import type { BrowserState } from '../shared/types';

/**
 * Writes a file so it is either the old version or the new one, never half of each:
 * the data goes to a temporary file that then replaces the real one in a single rename.
 * A crash or forced quit mid-save can't leave an empty state file (and lose every tab).
 */
export function writeFileAtomic(file: string, data: string) {
  const temp = `${file}.${process.pid}.tmp`;
  fs.writeFileSync(temp, data);
  fs.renameSync(temp, file);
}

/** The browser state as a JSON file in a folder. A missing or corrupt file loads as nothing. */
export function stateFile(dir: string): StatePersistence {
  const file = path.join(dir, 'flecha-state.json');
  return {
    load() {
      try {
        return JSON.parse(fs.readFileSync(file, 'utf8')) as BrowserState;
      } catch {
        return undefined;
      }
    },
    save(json) {
      fs.mkdirSync(path.dirname(file), { recursive: true });
      writeFileAtomic(file, json);
    },
  };
}
