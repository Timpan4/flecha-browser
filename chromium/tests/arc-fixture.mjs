// A synthetic Arc install, built from the format in docs/arc-import-format.md. No real profile data.
import fs from 'node:fs/promises';
import path from 'node:path';

/** Seconds since 2001, as Arc stores times, for a date. */
export const appleSeconds = (iso) => Date.parse(iso) / 1000 - 978307200;

const tab = (id, url, title) => [id, { id, title: null, data: { tab: { savedURL: url, savedTitle: title } }, childrenIds: [] }];
const node = (id, data, childrenIds, title = null) => [id, { id, title, data, childrenIds }];

/**
 * Two Spaces: Personal (Arc's default profile) and Work (Profile 1, with the newer
 * `newContainerIDs` and object markers), favorites for each profile, and three splits.
 */
export function arcSidebar() {
  const items = [
    ...tab('docs', 'https://docs.example.com/', 'Docs'),
    ...tab('repo', 'https://repo.example.com/', 'Repo'),
    ...tab('wiki', 'https://wiki.example.com/', 'Wiki'),
    ...node('dev', { list: {} }, ['repo', 'wiki'], 'Dev'),
    ...tab('left', 'https://left.example.com/', 'Left'),
    ...tab('right', 'https://right.example.com/', 'Right'),
    ...node('pinned-split', { splitView: {} }, ['left', 'right']),
    ...tab('settings', 'arc://settings', 'Arc settings'),
    ...node('board', { easel: {} }, []),
    ...node('p-pinned', { itemContainer: { containerType: { spaceItems: {} } } }, ['docs', 'dev', 'pinned-split', 'settings', 'board']),
    ...tab('news', 'https://news.example.com/', 'News'),
    ...tab('a', 'https://a.example.com/', 'A'),
    ...tab('b', 'https://b.example.com/', 'B'),
    ...tab('c', 'https://c.example.com/', 'C'),
    ...node('three-split', { splitView: {} }, ['a', 'b', 'c']),
    ...node('p-unpinned', { itemContainer: { containerType: { spaceItems: {} } } }, ['news', 'three-split']),
    ...tab('work', 'https://work.example.com/', 'Work home'),
    ...node('w-pinned', { itemContainer: { containerType: { spaceItems: {} } } }, ['work']),
    ...tab('w1', 'https://w1.example.com/', 'W1'),
    ...tab('w2', 'https://w2.example.com/', 'W2'),
    ...tab('w3', 'https://w3.example.com/', 'W3'),
    ...tab('w4', 'https://w4.example.com/', 'W4'),
    ...node('split-1', { splitView: {} }, ['w1', 'w2']),
    ...node('split-2', { splitView: {} }, ['w3', 'w4']),
    ...node('w-unpinned', { itemContainer: { containerType: { spaceItems: {} } } }, ['split-1', 'split-2']),
    ...tab('fav-mail', 'https://mail.example.com/', 'Mail'),
    ...tab('fav-cal', 'https://calendar.example.com/', 'Calendar'),
    ...tab('fav-mail-again', 'https://mail.example.com/', 'Mail'),
    ...node('fav-default', { itemContainer: { containerType: { topApps: {} } } }, ['fav-mail']),
    ...node('fav-work', { itemContainer: { containerType: { topApps: {} } } }, ['fav-cal', 'fav-mail-again']),
  ];
  const spaces = [
    'personal',
    {
      id: 'personal',
      title: 'Personal',
      profile: { default: true },
      containerIDs: ['pinned', 'p-pinned', 'unpinned', 'p-unpinned'],
      customInfo: { iconType: { emoji_v2: '🏠' }, windowTheme: { primaryColorPalette: { midTone: { red: 0.2, green: 0.4, blue: 0.6, alpha: 1, colorSpace: 'extendedSRGB' } } } },
    },
    'work',
    {
      id: 'work',
      title: 'Work',
      profile: { custom: { _0: { directoryBasename: 'Profile 1', machineID: 'test-machine' } } },
      newContainerIDs: [{ unpinned: {} }, 'w-unpinned', { pinned: {} }, 'w-pinned'],
      // Extended sRGB goes past 0 and 1.
      customInfo: { iconType: { emoji_v2: '💼' }, windowTheme: { primaryColorPalette: { midTone: { red: 1.2, green: -0.1, blue: 0.5, alpha: 1 } } } },
    },
  ];
  const topAppsContainerIDs = [{ default: true }, 'fav-default', { custom: { _0: { directoryBasename: 'Profile 1' } } }, 'fav-work'];
  return { version: 1, sidebar: { containers: [{ global: {} }, { spaces, items, topAppsContainerIDs }] } };
}

/** The same address archived twice (renamed the second time) and a page that isn't on the web. */
export function arcArchive() {
  return {
    items: [
      'x1', { archivedAt: appleSeconds('2025-05-01T10:00:00Z'), reason: 'auto', sidebarItem: { data: { tab: { savedURL: 'https://old.example.com/', savedTitle: 'Old' } } }, source: { space: { _0: 'personal' } } },
      'x2', { archivedAt: appleSeconds('2025-06-01T10:00:00Z'), reason: 'manual', sidebarItem: { title: 'Old, renamed', data: { tab: { savedURL: 'https://old.example.com/', savedTitle: 'Old' } } }, source: { space: { _0: 'personal' } } },
      'x3', { archivedAt: appleSeconds('2025-04-01T10:00:00Z'), reason: 'auto', sidebarItem: { data: { tab: { savedURL: 'https://older.example.com/', savedTitle: 'Older' } } }, source: { space: { _0: 'work' } } },
      'x4', { archivedAt: appleSeconds('2025-07-01T10:00:00Z'), reason: 'auto', sidebarItem: { data: { tab: { savedURL: 'arc://history' } } } },
    ],
  };
}

export const localState = { profile: { info_cache: { Default: { name: 'Person 1' }, 'Profile 1': { name: 'Work profile' } } } };

/** Writes the fixture as Arc lays it out in `dir`; returns the files written. */
export async function writeArc(dir, { archive = JSON.stringify(arcArchive()) } = {}) {
  await fs.mkdir(path.join(dir, 'User Data'), { recursive: true });
  const files = {
    sidebar: path.join(dir, 'StorableSidebar.json'),
    archive: path.join(dir, 'StorableArchiveItems.json'),
    localState: path.join(dir, 'User Data', 'Local State'),
  };
  await fs.writeFile(files.sidebar, JSON.stringify(arcSidebar()));
  await fs.writeFile(files.archive, archive);
  await fs.writeFile(files.localState, JSON.stringify(localState));
  return files;
}
