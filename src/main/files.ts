import fs from 'node:fs';

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
