import type { IconName } from './icons.js';
import type { FlechaApi, SearchEngineId, UiState } from '../shared/types';

/**
 * The first-run tour, drawn over the whole window: bring your data over, pick a look,
 * meet the features that make Flecha worth switching to, and make it the default.
 * It comes back from Help → Welcome to Flecha.
 */

type H = (tag: string, props?: Record<string, unknown>, ...children: Array<Node | string | null | undefined>) => HTMLElement;
type DetectedBrowser = Awaited<ReturnType<FlechaApi['detectBrowsers']>>[number];

const STEPS = ['welcome', 'import', 'look', 'features', 'default'] as const;
type Step = (typeof STEPS)[number];

const ENGINES: Array<[SearchEngineId, string]> = [
  ['google', 'Google'],
  ['duckduckgo', 'DuckDuckGo'],
  ['kagi', 'Kagi'],
  ['brave', 'Brave Search'],
  ['bing', 'Bing'],
  ['ecosia', 'Ecosia'],
];

const FEATURES: Array<{ icon: IconName; title: string; text: string; keys?: string }> = [
  { icon: 'search', title: 'Command bar', text: 'Tabs, history, actions and dev tricks (JWTs, timestamps, GitHub) in one box.', keys: 'newTab' },
  { icon: 'columns', title: 'Split by dragging', text: 'Drop a tab on the page to put it side by side. Drag the divider to resize.' },
  { icon: 'external', title: 'Peek', text: 'Links from pinned tabs open over the page, not in yet another tab.' },
  { icon: 'terminal', title: 'Local servers', text: 'Every dev server on your Mac shows up in the sidebar, console errors included.' },
  { icon: 'zap', title: 'Boosts', text: 'Your own TypeScript and CSS for any site, as files you keep in git.' },
  { icon: 'sparkle', title: 'Coding agents', text: 'Let Claude Code or Cursor see and drive your tabs over MCP. Off until you say so.' },
];

/** The Flecha mark: the arrowhead on its ink squircle. */
const MARK = `<svg viewBox="0 0 100 100" class="ob-mark-svg"><rect width="100" height="100" rx="30" fill="#0b2545"/><path d="M50 10 80 76 50 61 20 76Z" fill="#eef2f6" transform="translate(50 50) scale(0.62) rotate(45) translate(-50 -50)"/></svg>`;

export function setupOnboarding(api: FlechaApi, h: H, icon: (name: IconName) => SVGElement) {
  const root = document.getElementById('onboarding')!;
  let step: Step = 'welcome';
  let state: UiState | undefined;
  let browsers: DetectedBrowser[] | undefined;
  const imported = new Map<string, string>();
  const importing = new Set<string>();

  const go = (next: Step) => {
    step = next;
    draw();
  };
  const next = () => go(STEPS[Math.min(STEPS.indexOf(step) + 1, STEPS.length - 1)]);
  const back = () => go(STEPS[Math.max(STEPS.indexOf(step) - 1, 0)]);
  const finish = () => {
    api.finishOnboarding();
    api.openPalette('new-tab');
  };

  function dots() {
    return h(
      'div',
      { class: 'ob-dots' },
      ...STEPS.map((s, i) => h('span', { class: `ob-dot${s === step ? ' on' : ''}${i < STEPS.indexOf(step) ? ' done' : ''}` })),
    );
  }

  function footer(primary: string, onPrimary: () => void, secondary?: [string, () => void]) {
    return h(
      'footer',
      { class: 'ob-footer' },
      step === 'welcome' ? h('span') : h('button', { class: 'ob-link', onclick: back }, 'Back'),
      dots(),
      h(
        'span',
        { class: 'ob-actions' },
        secondary ? h('button', { class: 'ob-link', onclick: secondary[1] }, secondary[0]) : null,
        h('button', { class: 'ob-primary', onclick: onPrimary }, primary),
      ),
    );
  }

  function welcome() {
    const mark = h('div', { class: 'ob-mark' });
    mark.innerHTML = MARK;
    return h(
      'div',
      { class: 'ob-step ob-welcome' },
      h('div', { class: 'ob-hero' }, mark, h('h1', {}, 'Meet Flecha'), h('p', { class: 'ob-lead' }, 'A calm, fast browser for people who build things. Spaces, split view and a command bar for everything, plus tools for the web you make, not just the web you read.')),
      footer('Get started', next),
    );
  }

  function importStep() {
    if (!browsers) {
      api.detectBrowsers().then((found) => {
        browsers = found;
        if (step === 'import') draw();
      }, () => {
        browsers = [];
        if (step === 'import') draw();
      });
    }
    const list = !browsers
      ? h('div', { class: 'ob-loading' }, 'Looking for your browsers…')
      : !browsers.length
        ? h('div', { class: 'ob-loading' }, 'No other browsers found on this Mac. You can import later from File → Import Browser Data.')
        : h(
            'div',
            { class: 'ob-list' },
            ...browsers.map((b) => {
              const done = imported.get(b.id);
              const busy = importing.has(b.id);
              const what = [b.hasBookmarks && 'bookmarks', b.hasHistory && 'history'].filter(Boolean).join(' & ') || 'data';
              return h(
                'div',
                { class: `ob-browser${done ? ' done' : ''}` },
                h('span', { class: 'ob-browser-icon', style: `--hue: ${hue(b.name)}` }, b.name[0]),
                h('span', { class: 'ob-browser-text' }, h('strong', {}, b.name), h('small', {}, done ?? `${what}${b.profiles > 1 ? ` · ${b.profiles} profiles` : ''}`)),
                done
                  ? h('span', { class: 'ob-check' }, icon('check'))
                  : h(
                      'button',
                      {
                        class: 'ob-secondary',
                        disabled: busy,
                        onclick: async () => {
                          importing.add(b.id);
                          draw();
                          try {
                            const r = await api.importBrowser(b.id);
                            const parts = [r.bookmarks && `${r.bookmarks.toLocaleString()} bookmarks`, r.history && `${r.history.toLocaleString()} pages`].filter(Boolean);
                            imported.set(b.id, parts.length ? `Imported ${parts.join(' and ')}` : r.warnings[0] ?? 'Nothing to import');
                          } catch {
                            imported.set(b.id, 'Couldn’t import this one');
                          }
                          importing.delete(b.id);
                          draw();
                        },
                      },
                      busy ? 'Importing…' : 'Import',
                    ),
              );
            }),
          );
    return h(
      'div',
      { class: 'ob-step' },
      h('header', { class: 'ob-head' }, h('span', { class: 'ob-kicker' }, 'Step 1'), h('h2', {}, 'Bring everything with you'), h('p', { class: 'ob-lead' }, 'Bookmarks become pinned tabs in a Space of their own, and your history makes the command bar smart from day one. Passwords stay where they are.')),
      list,
      footer(imported.size ? 'Continue' : 'Skip for now', next),
    );
  }

  function look() {
    const theme = state?.theme ?? 'system';
    const engine = state?.searchEngine ?? 'google';
    return h(
      'div',
      { class: 'ob-step' },
      h('header', { class: 'ob-head' }, h('span', { class: 'ob-kicker' }, 'Step 2'), h('h2', {}, 'Make it yours'), h('p', { class: 'ob-lead' }, 'Each Space gets its own colour later on. For now, light or dark?')),
      h(
        'div',
        { class: 'ob-themes' },
        ...(['system', 'light', 'dark'] as const).map((t) =>
          h(
            'button',
            { class: `ob-theme${theme === t ? ' selected' : ''}`, onclick: () => api.setPreferences({ theme: t }) },
            h('span', { class: `ob-theme-preview ${t}` }, h('span'), h('span')),
            h('span', {}, t === 'system' ? 'Match macOS' : t === 'light' ? 'Light' : 'Dark'),
          ),
        ),
      ),
      h(
        'div',
        { class: 'ob-field' },
        h('span', {}, 'Search with'),
        h(
          'div',
          { class: 'ob-chips' },
          ...ENGINES.map(([id, name]) => h('button', { class: `ob-chip${engine === id ? ' selected' : ''}`, onclick: () => api.setPreferences({ searchEngine: id }) }, name)),
        ),
      ),
      h(
        'label',
        { class: 'ob-toggle' },
        h('span', { class: 'ob-toggle-icon' }, icon('shield')),
        h('span', { class: 'ob-toggle-text' }, h('strong', {}, 'Block ads and trackers'), h('small', {}, 'EasyList, EasyPrivacy and uBlock filters, built in.')),
        (() => {
          const box = h('input', { type: 'checkbox', class: 'switch' }) as HTMLInputElement;
          box.checked = state?.adBlock !== false;
          box.addEventListener('change', () => api.setPreferences({ adBlock: box.checked }));
          return box;
        })(),
      ),
      footer('Continue', next),
    );
  }

  function features() {
    return h(
      'div',
      { class: 'ob-step' },
      h('header', { class: 'ob-head' }, h('span', { class: 'ob-kicker' }, 'Step 3'), h('h2', {}, 'What makes Flecha different'), h('p', { class: 'ob-lead' }, 'The things you’d otherwise need five extensions for.')),
      h(
        'div',
        { class: 'ob-features' },
        ...FEATURES.map((f, i) =>
          h(
            'div',
            { class: 'ob-feature', style: `animation-delay: ${i * 50}ms` },
            h('span', { class: 'ob-feature-icon' }, icon(f.icon)),
            h('strong', {}, f.title, f.keys && state?.shortcuts[f.keys] ? h('kbd', {}, state.shortcuts[f.keys]) : null),
            h('p', {}, f.text),
          ),
        ),
      ),
      footer('Continue', next),
    );
  }

  function makeDefault() {
    const isDefault = !!state?.defaultBrowser?.isDefault;
    return h(
      'div',
      { class: 'ob-step ob-welcome' },
      h(
        'div',
        { class: 'ob-hero' },
        h('span', { class: `ob-big-icon${isDefault ? ' done' : ''}` }, icon(isDefault ? 'check' : 'link')),
        h('h2', {}, isDefault ? 'You’re all set' : 'Open links in Flecha'),
        h('p', { class: 'ob-lead' }, isDefault ? 'Links from Slack, Mail and your terminal now open here.' : 'Make Flecha your default browser so links from Slack, Mail and your terminal land here.'),
        isDefault || !state?.defaultBrowser?.canSet ? null : h('button', { class: 'ob-secondary big', onclick: () => api.makeDefaultBrowser() }, 'Make Default Browser'),
      ),
      footer('Start browsing', finish),
    );
  }

  function draw() {
    const view = { welcome, import: importStep, look, features, default: makeDefault }[step]();
    root.replaceChildren(h('div', { class: 'ob-card' }, view));
  }

  let drawn = '';
  return function render(s: UiState) {
    state = s;
    const show = !s.onboarded;
    if (show && root.hidden) step = 'welcome';
    root.hidden = !show;
    // Only what the steps show; other state updates (tabs loading, servers) would restart the animations.
    const key = JSON.stringify([s.theme, s.searchEngine, s.adBlock, s.defaultBrowser, s.shortcuts.newTab]);
    if (show && (key !== drawn || !root.firstChild)) draw();
    drawn = key;
  };
}

function hue(text: string) {
  let hash = 0;
  for (const char of text) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return hash % 360;
}
