import type { FindBarApi } from '../shared/types';

declare global {
  interface Window {
    findBar: FindBarApi;
  }
}

const api = window.findBar;
const input = document.getElementById('query') as HTMLInputElement;
const count = document.getElementById('count')!;

function search(forward: boolean, newSearch: boolean) {
  api.find(input.value, forward, newSearch);
}

input.addEventListener('input', () => search(true, true));
input.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') api.close();
  else if (e.key === 'Enter') {
    e.preventDefault();
    search(!e.shiftKey, false);
  }
});
document.getElementById('bar')!.addEventListener('submit', (e) => e.preventDefault());
document.getElementById('next')!.onclick = () => search(true, false);
document.getElementById('prev')!.onclick = () => search(false, false);
document.getElementById('close')!.onclick = () => api.close();

api.onOpen(() => {
  input.focus();
  input.select();
  // Reopening with text already there searches again, like Chrome.
  if (input.value) search(true, true);
});

api.onResult(({ active, total }) => {
  count.textContent = input.value ? (total ? `${active} of ${total}` : 'No results') : '';
  count.classList.toggle('none', !!input.value && !total);
});
