import type { DividerApi } from '../shared/types';

declare global {
  interface Window {
    divider: DividerApi;
  }
}

const api = window.divider;
let dragging = false;

document.addEventListener('pointerdown', (e) => {
  if (e.detail === 2) return api.reset();
  dragging = true;
  document.body.classList.add('dragging');
  document.body.setPointerCapture(e.pointerId);
  api.start();
});

document.addEventListener('pointermove', (e) => {
  if (dragging) api.drag(e.clientX);
});

const stop = () => {
  if (!dragging) return;
  dragging = false;
  document.body.classList.remove('dragging');
  api.end();
};
document.addEventListener('pointerup', stop);
document.addEventListener('pointercancel', stop);
