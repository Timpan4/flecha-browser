/**
 * Extension buttons in the sidebar. Flecha hides Chromium's toolbar, so the frame page shows
 * the toolbar's extension actions instead (patch 0056) and runs them: a popup opens below its
 * button, and right-click shows the extension's own menu (options, pin, remove). The puzzle
 * button after them opens Chromium's extensions menu there (patch 0071).
 */
const surface = document.documentElement.dataset.surface;
if (surface === 'window' && typeof chrome.flechaPrivate?.getExtensionActions === 'function') void showExtensionActions();

async function showExtensionActions() {
  const windowId = await chrome.flechaPrivate.getWindowId();
  const list = document.getElementById('extensions')!;
  const menuButton = document.getElementById('btn-extensions')!;
  const anchor = (button: HTMLElement) => {
    const r = button.getBoundingClientRect();
    return { x: Math.round(r.left), y: Math.round(r.top), width: Math.round(r.width), height: Math.round(r.height) };
  };
  // A browser without patch 0071 has no extensions menu to show.
  menuButton.hidden = typeof chrome.flechaPrivate.showExtensionsMenu !== 'function';
  menuButton.addEventListener('click', () => void chrome.flechaPrivate.showExtensionsMenu(windowId, anchor(menuButton)));
  // One read at a time, and one more if something changed meanwhile: badges can change often.
  let reading: Promise<void> | undefined;
  let again = false;
  const refresh = () => {
    if (reading) return void (again = true);
    reading = chrome.flechaPrivate
      .getExtensionActions(windowId)
      // Flecha's own action (the stock-Chromium side panel's button) is this sidebar.
      .then((all) => all.filter((action) => action.id !== chrome.runtime.id))
      .then((actions) => {
        list.replaceChildren(
          ...actions.map((action) => {
            const button = document.createElement('button');
            button.className = 'ext-action';
            button.classList.toggle('disabled', !action.enabled);
            button.dataset.tip = action.tooltip || action.name;
            button.setAttribute('aria-label', action.name);
            if (action.icon) button.append(Object.assign(document.createElement('img'), { src: action.icon, alt: '' }));
            // A disabled action shows the extension's menu at its button, as on the toolbar.
            button.addEventListener('click', () => void chrome.flechaPrivate.runExtensionAction(windowId, action.id, anchor(button)));
            button.addEventListener('contextmenu', (e) => {
              e.preventDefault();
              void chrome.flechaPrivate.showExtensionActionMenu(windowId, action.id, anchor(button));
            });
            return button;
          }),
          menuButton,
        );
        list.hidden = !actions.length;
      })
      .catch((err) => console.error('Flecha shell: extension actions failed', err))
      .finally(() => {
        reading = undefined;
        if (again) {
          again = false;
          refresh();
        }
      });
  };
  chrome.flechaPrivate.onExtensionActionsChanged.addListener((id) => id === windowId && refresh());
  // Icons and badges are per tab.
  chrome.tabs.onActivated.addListener((info) => info.windowId === windowId && refresh());
  refresh();
}
