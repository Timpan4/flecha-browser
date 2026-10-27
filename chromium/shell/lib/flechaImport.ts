/**
 * Flecha (Electron)'s state to import, or undefined. Flecha's native host reads it straight from
 * disk; the file picker is for when it isn't installed. Either way it replaces every Space, pin
 * and folder, so you're asked first.
 */
export async function chooseFlechaState(from: {
  readNative(): Promise<string | undefined>;
  pickFile(): Promise<string | undefined>;
  confirm(message: string): boolean;
}): Promise<string | undefined> {
  const json = (await from.readNative()) ?? (await from.pickFile());
  if (!json) return undefined;
  const ok = from.confirm('Replace your Spaces and tabs with those of Flecha (Electron)? Yours are kept: “Restore Spaces From Before Import” in the command bar brings them back.');
  return ok ? json : undefined;
}
