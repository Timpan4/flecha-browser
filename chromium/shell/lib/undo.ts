import { isMac } from './platform';

/** Undoes the last Space deletion, bulk close or bulk move, in the sidebar and in Settings. */
export const isUndoKey = (e: { key: string; metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean }) =>
  e.key.toLowerCase() === 'z' && (isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey) && !e.altKey && !e.shiftKey;

/** What deleting a Space does, for the question before it. */
export const DELETE_SPACE_DETAIL = `Its pinned and today tabs are closed and archived. ${isMac ? '⌘Z' : 'Ctrl+Z'} in the sidebar, or Undo in the command bar, brings it all back.`;
