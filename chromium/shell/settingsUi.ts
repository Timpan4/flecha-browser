/**
 * What Electron draws natively and an extension page has to draw itself: context menus (the
 * Tabs & Spaces menus in Settings, the sidebar's tab and Space menus in the window), the
 * "Delete …?" questions, and a note for what isn't ported yet. Styles are in shell.css.
 */
import type { AskOptions } from './lib/tabManager';

/** A menu as data, like Electron's menu templates: items carry whatever `run` the caller picks. */
export type MenuEntry<T> =
  /** `accel` is the item's shortcut as shown ("Ctrl+H"). */
  | { label: string; run: T; enabled?: boolean; checked?: boolean; accel?: string }
  | { label: string; submenu: MenuEntry<T>[]; enabled?: boolean }
  | { separator: true };
type Picked<T> = { label: string; run: T };

export interface MenuOptions {
  /** Opens below this element (a menu button) instead of at the pointer. */
  anchor?: Element;
  /** Opened from the keyboard: the first item is highlighted at once. */
  keyboard?: boolean;
}

// Menus open where the pointer last was, as native context menus do.
let pointer = { x: 0, y: 0 };
for (const type of ['pointerdown', 'contextmenu'] as const) {
  window.addEventListener(type, (e) => (pointer = { x: e.clientX, y: e.clientY }), true);
}

let closeOpenMenu: (() => void) | undefined;

/**
 * Shows the menu at the pointer (or below `options.anchor`); resolves with the item picked, or
 * undefined if dismissed. It behaves like a macOS menu: one highlighted item, which the pointer
 * and the arrow keys (Home and End too) both move, and which has keyboard focus, so a screen
 * reader reads it; submenus open beside their item, on hover or →, and ← or Esc goes back to
 * that item; Esc closes.
 */
export function popupMenu<T>(items: MenuEntry<T>[], options: MenuOptions = {}): Promise<Picked<T> | undefined> {
  closeOpenMenu?.();
  return new Promise((resolve) => {
    const layer = document.createElement('div');
    layer.className = 'shell-menu-layer';
    /** The open menus, the root first; each remembers its highlighted row. */
    const stack: Array<{ menu: HTMLElement; rows: HTMLButtonElement[]; active?: HTMLButtonElement }> = [];
    let openTimer: number | undefined;

    const close = (picked?: Picked<T>) => {
      clearTimeout(openTimer);
      layer.remove();
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('blur', dismiss);
      closeOpenMenu = undefined;
      resolve(picked);
    };
    const dismiss = () => close();

    /** Closes the submenus deeper than `level`. */
    const closeBelow = (level: number) => {
      while (stack.length > level + 1) stack.pop()!.menu.remove();
      for (const row of stack[level].rows) {
        row.classList.remove('open');
        if (row.hasAttribute('aria-expanded')) row.setAttribute('aria-expanded', 'false');
      }
    };
    /** Closes the submenu open at `level + 1` from the keyboard: its item gets focus back. */
    const leaveSubmenu = (level: number) => {
      closeBelow(level);
      highlight(level, stack[level].active);
    };

    const highlight = (level: number, row: HTMLButtonElement | undefined) => {
      const entry = stack[level];
      entry.active?.classList.remove('active');
      entry.active = row;
      row?.classList.add('active');
      row?.scrollIntoView({ block: 'nearest' });
      row?.focus({ preventScroll: true });
    };

    const openSubmenu = (level: number, row: HTMLButtonElement, entries: MenuEntry<T>[], focusFirst: boolean) => {
      if (stack[level + 1]?.menu.dataset.parent === row.dataset.key) return;
      closeBelow(level);
      row.classList.add('open');
      row.setAttribute('aria-expanded', 'true');
      const sub = build(entries, level + 1);
      sub.dataset.parent = row.dataset.key;
      sub.setAttribute('aria-label', row.firstChild?.textContent ?? '');
      layer.append(sub);
      // Beside its item, flipped to the other side when there's no room.
      const at = row.getBoundingClientRect();
      const { width, height } = sub.getBoundingClientRect();
      const right = at.right + 2;
      sub.style.left = `${right + width <= innerWidth - 4 ? right : Math.max(4, at.left - width - 2)}px`;
      sub.style.top = `${Math.max(4, Math.min(at.top - 5, innerHeight - height - 4))}px`;
      if (focusFirst) highlight(level + 1, stack[level + 1].rows[0]);
    };

    let keys = 0;
    const build = (entries: MenuEntry<T>[], level: number): HTMLElement => {
      const menu = document.createElement('div');
      menu.className = 'shell-menu';
      menu.setAttribute('role', 'menu');
      const rows: HTMLButtonElement[] = [];
      stack[level] = { menu, rows };
      for (const item of entries) {
        if ('separator' in item) {
          menu.append(Object.assign(document.createElement('div'), { className: 'shell-menu-separator' }));
          continue;
        }
        const row = Object.assign(document.createElement('button'), { className: 'shell-menu-item', textContent: item.label, tabIndex: -1 });
        if ('accel' in item && item.accel) {
          row.append(Object.assign(document.createElement('span'), { className: 'shell-menu-accel', textContent: item.accel }));
        }
        row.dataset.key = String(keys++);
        row.setAttribute('role', 'menuitem');
        menu.append(row);
        if (item.enabled === false) {
          row.disabled = true;
          continue;
        }
        rows.push(row);
        if ('checked' in item && item.checked !== undefined) {
          row.setAttribute('role', 'menuitemcheckbox');
          row.setAttribute('aria-checked', String(item.checked));
          row.classList.add('checkable');
        }
        const submenu = 'submenu' in item ? item.submenu : undefined;
        if (submenu) {
          row.classList.add('has-submenu');
          row.setAttribute('aria-haspopup', 'menu');
          row.setAttribute('aria-expanded', 'false');
        }
        row.addEventListener('pointerenter', () => {
          highlight(level, row);
          clearTimeout(openTimer);
          // A short pause, so passing over an item on the way to its neighbour opens nothing.
          openTimer = window.setTimeout(() => (submenu ? openSubmenu(level, row, submenu, false) : closeBelow(level)), 120);
        });
        row.addEventListener('click', () => {
          if (submenu) openSubmenu(level, row, submenu, false);
          else close(item as Picked<T>);
        });
        row.addEventListener('mousedown', (e) => e.preventDefault());
      }
      return menu;
    };

    const onKey = (e: KeyboardEvent) => {
      const level = stack.length - 1;
      const { rows, active } = stack[level];
      const i = active ? rows.indexOf(active) : -1;
      const handled = () => {
        e.preventDefault();
        e.stopImmediatePropagation();
      };
      if (e.key === 'Escape') {
        handled();
        if (level > 0) {
          leaveSubmenu(level - 1);
        } else close();
      } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        handled();
        const step = e.key === 'ArrowDown' ? 1 : -1;
        highlight(level, rows[(i + step + rows.length) % rows.length] ?? rows[0]);
      } else if (e.key === 'Home' || e.key === 'End') {
        handled();
        highlight(level, e.key === 'Home' ? rows[0] : rows[rows.length - 1]);
      } else if (e.key === 'Tab') {
        // Like a native menu, Tab leaves it.
        handled();
        close();
      } else if (e.key === 'ArrowRight' && active?.classList.contains('has-submenu')) {
        handled();
        active.click();
        if (stack[level + 1]) highlight(level + 1, stack[level + 1].rows[0]);
      } else if (e.key === 'ArrowLeft' && level > 0) {
        handled();
        leaveSubmenu(level - 1);
      } else if (e.key === 'Enter' || e.key === ' ') {
        handled();
        if (active?.classList.contains('has-submenu')) {
          active.click();
          if (stack[level + 1]) highlight(level + 1, stack[level + 1].rows[0]);
        } else active?.click();
      }
    };

    layer.addEventListener('pointerdown', (e) => e.target === layer && close());
    layer.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      if (e.target === layer) close();
    });
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('blur', dismiss);
    closeOpenMenu = dismiss;

    const menu = build(items, 0);
    layer.append(menu);
    document.body.append(layer);
    // Below its button, or at the pointer; on screen either way.
    const anchor = options.anchor?.getBoundingClientRect();
    const at = anchor ? { x: anchor.left, y: anchor.bottom + 4 } : pointer;
    const { width, height } = menu.getBoundingClientRect();
    menu.style.left = `${Math.max(4, Math.min(at.x, innerWidth - width - 4))}px`;
    menu.style.top = `${Math.max(4, Math.min(at.y, innerHeight - height - 4))}px`;
    if (options.anchor) menu.setAttribute('aria-label', options.anchor.getAttribute('aria-label') ?? '');
    // Like a native menu, nothing is highlighted until the pointer or a key says so (or it was
    // opened from the keyboard); the layer takes focus so the arrow keys reach it.
    layer.tabIndex = -1;
    layer.focus();
    if (options.keyboard) highlight(0, stack[0].rows[0]);
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
