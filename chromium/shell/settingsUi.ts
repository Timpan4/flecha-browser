/**
 * What Electron draws natively for Settings and an extension page has to draw itself: the
 * Tabs & Spaces menus, the "Delete …?" questions, and a note for what isn't ported yet.
 * Styles are in settings-shell.css.
 */
import type { AskOptions, TabsMenuItem } from '../../src/main/tabManager';

// Menus open where the pointer last was, as native context menus do.
let pointer = { x: 0, y: 0 };
for (const type of ['pointerdown', 'contextmenu'] as const) {
  window.addEventListener(type, (e) => (pointer = { x: e.clientX, y: e.clientY }), true);
}

let closeOpenMenu: (() => void) | undefined;

/** Shows the menu at the pointer; resolves with the item picked, or undefined if dismissed. */
export function popupMenu(items: TabsMenuItem[]): Promise<Extract<TabsMenuItem, { run: unknown }> | undefined> {
  closeOpenMenu?.();
  return new Promise((resolve) => {
    const layer = document.createElement('div');
    layer.className = 'shell-menu-layer';
    const close = (picked?: Extract<TabsMenuItem, { run: unknown }>) => {
      layer.remove();
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('blur', dismiss);
      closeOpenMenu = undefined;
      resolve(picked);
    };
    const dismiss = () => close();
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      e.stopImmediatePropagation();
      close();
    };
    layer.addEventListener('pointerdown', (e) => e.target === layer && close());
    layer.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      if (e.target === layer) close();
    });
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('blur', dismiss);
    closeOpenMenu = dismiss;

    const build = (entries: TabsMenuItem[]): HTMLElement => {
      const menu = document.createElement('div');
      menu.className = 'shell-menu';
      menu.setAttribute('role', 'menu');
      for (const item of entries) {
        if ('separator' in item) {
          menu.append(Object.assign(document.createElement('div'), { className: 'shell-menu-separator' }));
          continue;
        }
        const row = Object.assign(document.createElement('button'), { className: 'shell-menu-item', textContent: item.label });
        row.setAttribute('role', 'menuitem');
        if ('submenu' in item) {
          row.classList.add('has-submenu');
          const sub = build(item.submenu);
          sub.classList.add('shell-submenu');
          const wrap = document.createElement('div');
          wrap.className = 'shell-menu-branch';
          wrap.append(row, sub);
          row.addEventListener('click', () => wrap.classList.toggle('open'));
          menu.append(wrap);
        } else {
          row.addEventListener('click', () => close(item));
          menu.append(row);
        }
      }
      return menu;
    };

    const menu = build(items);
    layer.append(menu);
    document.body.append(layer);
    // Keep the menu on screen.
    const { width, height } = menu.getBoundingClientRect();
    menu.style.left = `${Math.max(4, Math.min(pointer.x, innerWidth - width - 4))}px`;
    menu.style.top = `${Math.max(4, Math.min(pointer.y, innerHeight - height - 4))}px`;
    menu.querySelector<HTMLButtonElement>('.shell-menu-item')?.focus();
  });
}

/** Asks with buttons, like Electron's message box; resolves with the index of the button chosen. */
export function askDialog(options: AskOptions): Promise<number> {
  return new Promise((resolve) => {
    const dialog = document.createElement('dialog');
    dialog.className = 'shell-dialog';
    const message = Object.assign(document.createElement('strong'), { textContent: options.message });
    const detail = Object.assign(document.createElement('p'), { textContent: options.detail });
    const buttons = document.createElement('div');
    buttons.className = 'shell-dialog-buttons';
    let answer = options.cancelId;
    options.buttons.forEach((label, i) => {
      const button = Object.assign(document.createElement('button'), { className: 'button', textContent: label });
      if (i === options.defaultId) button.classList.add('primary');
      if (/^Delete/.test(label)) button.classList.add('danger');
      button.addEventListener('click', () => {
        answer = i;
        dialog.close();
      });
      buttons.append(button);
    });
    // Esc cancels.
    dialog.addEventListener('close', () => {
      dialog.remove();
      resolve(answer);
    });
    dialog.append(message, detail, buttons);
    document.body.append(dialog);
    dialog.showModal();
    (buttons.children[options.defaultId] as HTMLButtonElement | undefined)?.focus();
  });
}

let toastTimer: number | undefined;

/** A short note at the bottom of the window. */
export function toast(text: string) {
  let el = document.querySelector<HTMLElement>('.shell-toast');
  if (!el) {
    el = Object.assign(document.createElement('div'), { className: 'shell-toast' });
    el.setAttribute('role', 'status');
    document.body.append(el);
  }
  el.textContent = text;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (el!.hidden = true), 4000);
}
