/** Loads a Settings pane once, or lets the next visit retry a failed load. */
export function lazySettingsPane(load: () => Promise<void>) {
  let pending: Promise<void> | undefined;
  let error: HTMLElement | undefined;

  return (pane: HTMLElement, title: string) => pending ??= Promise.resolve().then(load).then(() => {
    pane.classList.remove('pane-load-failed');
    error?.remove();
    error = undefined;
  }).catch((cause) => {
    pending = undefined;
    console.error(`Could not load the ${title} settings pane`, cause);
    if (!error) {
      error = document.createElement('p');
      error.className = 'pane-load-error lead';
      error.setAttribute('role', 'alert');
      error.textContent = `Could not load ${title}. Open another pane, then return to try again.`;
      pane.classList.add('pane-load-failed');
      pane.prepend(error);
    }
  });
}
