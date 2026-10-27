import type { BrowserState, ShortcutInfo } from '../../shared/types';
import { isMac } from './platform';

/**
 * Every command with a keyboard shortcut. Menus, tooltips and Settings all read keys from
 * here, so a shortcut you record in Settings changes everywhere at once. Keys are Electron
 * accelerators ("CmdOrCtrl+Shift+T").
 */
export const SHORTCUTS = [
  { id: 'newTab', group: 'Getting around', label: 'New tab', keys: 'CmdOrCtrl+T' },
  { id: 'openLocation', group: 'Getting around', label: 'Open location', keys: 'CmdOrCtrl+L' },
  { id: 'back', group: 'Getting around', label: 'Back', keys: 'CmdOrCtrl+[' },
  { id: 'forward', group: 'Getting around', label: 'Forward', keys: 'CmdOrCtrl+]' },
  { id: 'reload', group: 'Getting around', label: 'Reload', keys: 'CmdOrCtrl+R' },
  { id: 'hardReload', group: 'Getting around', label: 'Hard reload', keys: 'CmdOrCtrl+Shift+R' },
  { id: 'find', group: 'Getting around', label: 'Find in page', keys: 'CmdOrCtrl+F' },
  { id: 'toggleSidebar', group: 'Getting around', label: 'Show or hide the sidebar', keys: 'CmdOrCtrl+S' },
  { id: 'settings', group: 'Getting around', label: 'Settings', keys: 'CmdOrCtrl+,' },
  { id: 'history', group: 'Getting around', label: 'History', keys: 'CmdOrCtrl+Y' },
  { id: 'newFlechinha', group: 'Tabs', label: 'New Flechinha window', keys: 'Alt+CmdOrCtrl+N' },
  { id: 'closeTab', group: 'Tabs', label: 'Close tab', keys: 'CmdOrCtrl+W' },
  { id: 'reopenTab', group: 'Tabs', label: 'Reopen closed tab', keys: 'CmdOrCtrl+Shift+T' },
  { id: 'nextTab', group: 'Tabs', label: 'Switch to recent tab', keys: 'Ctrl+Tab' },
  { id: 'previousTab', group: 'Tabs', label: 'Switch to recent tab, backwards', keys: 'Ctrl+Shift+Tab' },
  { id: 'pinTab', group: 'Tabs', label: 'Pin or unpin tab', keys: 'CmdOrCtrl+D' },
  { id: 'duplicateTab', group: 'Tabs', label: 'Duplicate tab', keys: 'Alt+CmdOrCtrl+D' },
  { id: 'muteTab', group: 'Tabs', label: 'Mute or unmute tab', keys: 'Ctrl+M' },
  { id: 'clearToday', group: 'Tabs', label: 'Clear Today', keys: 'CmdOrCtrl+Shift+K' },
  { id: 'copyLink', group: 'Tabs', label: 'Copy link', keys: 'CmdOrCtrl+Shift+C' },
  { id: 'copyMarkdown', group: 'Tabs', label: 'Copy as Markdown link', keys: 'Alt+CmdOrCtrl+C' },
  { id: 'screenshot', group: 'Tabs', label: 'Copy screenshot', keys: 'CmdOrCtrl+Shift+2' },
  { id: 'nextSpace', group: 'Spaces', label: 'Next Space', keys: 'Alt+CmdOrCtrl+Right' },
  { id: 'previousSpace', group: 'Spaces', label: 'Previous Space', keys: 'Alt+CmdOrCtrl+Left' },
  { id: 'newSpace', group: 'Spaces', label: 'New Space', keys: 'Ctrl+Shift+N' },
  { id: 'splitNew', group: 'Split & Peek', label: 'Split with a new tab', keys: 'Ctrl+Shift+=' },
  { id: 'exitSplit', group: 'Split & Peek', label: 'Exit split view', keys: 'Ctrl+Shift+-' },
  { id: 'zoomIn', group: 'View', label: 'Zoom in', keys: 'CmdOrCtrl+=' },
  { id: 'zoomOut', group: 'View', label: 'Zoom out', keys: 'CmdOrCtrl+-' },
  { id: 'actualSize', group: 'View', label: 'Actual size', keys: 'CmdOrCtrl+0' },
  { id: 'devTools', group: 'Developer', label: 'Developer Tools', keys: 'Alt+CmdOrCtrl+I' },
  { id: 'responsive', group: 'Developer', label: 'Responsive view', keys: 'Alt+CmdOrCtrl+M' },
  { id: 'nextEnvironment', group: 'Developer', label: 'Next environment', keys: 'Alt+CmdOrCtrl+E' },
  { id: 'editBoost', group: 'Developer', label: 'Edit boost for this site', keys: 'Alt+CmdOrCtrl+B' },
  { id: 'viewSource', group: 'Developer', label: 'View source', keys: 'Alt+CmdOrCtrl+U' },
  { id: 'shortcuts', group: 'Getting around', label: 'Keyboard shortcuts', keys: 'CmdOrCtrl+/' },
  { id: 'quit', group: 'Getting around', label: 'Quit Flecha', keys: 'Ctrl+Shift+Q' },
] as const;

export type ShortcutId = (typeof SHORTCUTS)[number]['id'];

/** Shortcuts that are a family of keys (⌘1…⌘9) and can't be recorded as one. */
export function fixedShortcuts(mac = isMac): Record<string, string> {
  const map: Record<string, string> = { 'CmdOrCtrl+O': 'openInFlecha' };
  for (let i = 1; i <= 9; i++) {
    map[`CmdOrCtrl+${i}`] = `selectTab:${i}`;
    map[`${mac ? 'Ctrl' : 'Ctrl+Alt'}+${i}`] = `selectSpace:${i}`;
  }
  return map;
}

export function defaultKeys(id: ShortcutId, mac = isMac): string {
  if (id === 'newSpace' && !mac) return 'Ctrl+Alt+Shift+N';
  // Chrome's keys for History: ⌘Y on macOS, Ctrl+H elsewhere.
  if (id === 'history' && !mac) return 'Ctrl+H';
  // macOS quits from its menu bar (⌘Q); elsewhere Flecha has no menu bar to hold the key.
  if (id === 'quit' && mac) return '';
  return SHORTCUTS.find((s) => s.id === id)!.keys;
}

/** The accelerator for a command: yours if you set one, none if you removed it, else the default. */
export function keysFor(state: BrowserState, id: ShortcutId, mac = isMac): string | undefined {
  const own = state.keybindings?.[id];
  if (own === null) return undefined;
  if (own) return Object.keys(fixedShortcuts(mac)).some((fixed) => sameKeys(own, fixed, mac)) ? undefined : own;
  const fallback = defaultKeys(id, mac);
  // Saved custom bindings can move between platforms. Keep the custom command when
  // CmdOrCtrl resolves differently, and leave any conflicting default unbound.
  if (SHORTCUTS.some((other) => other.id !== id && state.keybindings?.[other.id] && sameKeys(state.keybindings[other.id]!, fallback, mac))) return undefined;
  return fallback;
}

export function isShortcutId(id: string): id is ShortcutId {
  return SHORTCUTS.some((s) => s.id === id);
}

/** Two accelerators that press the same keys ("CmdOrCtrl+T" and "Cmd+T") compare equal. */
export function sameKeys(a: string, b: string, mac = isMac) {
  const norm = (keys: string) => acceleratorParts(keys, mac).sort().join('+').toLowerCase();
  return norm(a) === norm(b);
}

function acceleratorParts(keys: string, mac: boolean): string[] {
  const aliases: Record<string, string> = { CmdOrCtrl: mac ? 'Cmd' : 'Ctrl', CommandOrControl: mac ? 'Cmd' : 'Ctrl', Command: 'Cmd', Option: 'Alt', Control: 'Ctrl' };
  return keys.split('+').map((part) => aliases[part] ?? part);
}

/** Use the platform's familiar modifier labels in menus, hints and Settings. */
export function displayKeys(keys: string | undefined, mac = isMac) {
  if (!keys) return '';
  const parts = acceleratorParts(keys, mac);
  const mods = new Set(parts.slice(0, -1));
  const key = parts[parts.length - 1];
  if (!mac) return [...['Ctrl', 'Alt', 'Shift', 'Cmd'].filter((m) => mods.has(m)).map((m) => m === 'Cmd' ? 'Super' : m), key.length === 1 ? key.toUpperCase() : key].join('+');
  const names: Record<string, string> = { Left: '←', Right: '→', Up: '↑', Down: '↓', Return: '↩', Enter: '↩', Tab: '⇥', Space: 'Space', Escape: 'Esc', Backspace: '⌫', Delete: '⌦', '-': '−' };
  return (
    (mods.has('Ctrl') ? '⌃' : '') + (mods.has('Alt') ? '⌥' : '') + (mods.has('Shift') ? '⇧' : '') + (mods.has('Cmd') ? '⌘' : '') + (names[key] ?? key.toUpperCase())
  );
}

/** Everything Settings shows, in order, with your keys applied. */
export function shortcutList(state: BrowserState, mac = isMac): ShortcutInfo[] {
  const editable = SHORTCUTS.map((s): ShortcutInfo => {
    const keys = keysFor(state, s.id, mac);
    return { id: s.id, group: s.group, label: s.label, keys: displayKeys(keys, mac), changed: state.keybindings?.[s.id] !== undefined };
  });
  const fixed: ShortcutInfo[] = [
    { id: 'tabN', group: 'Tabs', label: 'Go to tab · last tab', keys: `${displayKeys('CmdOrCtrl+1', mac)} … ${displayKeys('CmdOrCtrl+9', mac)}`, fixed: true },
    { id: 'spaceN', group: 'Spaces', label: 'Go to Space', keys: `${displayKeys(`${mac ? 'Ctrl' : 'Ctrl+Alt'}+1`, mac)} … ${displayKeys(`${mac ? 'Ctrl' : 'Ctrl+Alt'}+9`, mac)}`, fixed: true },
    { id: 'closePeek', group: 'Split & Peek', label: 'Close Peek', keys: 'Esc', fixed: true },
  ];
  const groups = [...new Set(SHORTCUTS.map((s) => s.group))];
  return groups.flatMap((g) => [...editable.filter((s) => s.group === g), ...fixed.filter((s) => s.group === g)]);
}

/** The sidebar's tooltips and hints, by command: { newTab: '⌘T', … }. */
export function shortcutHints(state: BrowserState): Record<string, string> {
  return Object.fromEntries(SHORTCUTS.map((s) => [s.id, displayKeys(keysFor(state, s.id))]));
}
