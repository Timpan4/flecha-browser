/** Startup-only video diagnostics, with saved and active modes shown separately. */
import type { DebugSettings, SettingsApi } from '../../shared/types';

export function setupDebugPane(api: SettingsApi) {
  const pane = document.querySelector<HTMLElement>('.pane[data-pane="debug"]')!;
  const toggle = document.getElementById('debug-video-buffer') as HTMLInputElement;
  const status = document.getElementById('debug-video-buffer-status')!;
  let saved: DebugSettings | undefined;
  let writing = false;
  let stopped = false;
  let revision = 0;

  function render(state: DebugSettings) {
    saved = state;
    toggle.checked = state.disableGpuMemoryBufferVideoFrames || state.videoBufferWorkaroundForced;
    toggle.disabled = writing || state.videoBufferWorkaroundForced || !api.setVideoBufferWorkaround;
    if (state.videoBufferWorkaroundForced) {
      status.textContent = 'Enabled by a command-line flag. Remove --disable-gpu-memory-buffer-video-frames from your launcher to turn it off.';
    } else if (state.disableGpuMemoryBufferVideoFrames !== state.videoBufferWorkaroundActive) {
      status.textContent = `Saved. Restart Flecha to turn the workaround ${state.disableGpuMemoryBufferVideoFrames ? 'on' : 'off'}.`;
    } else {
      status.textContent = `Workaround is ${state.videoBufferWorkaroundActive ? 'on' : 'off'} in this browser session.`;
    }
  }

  async function refresh() {
    if (stopped || writing || pane.hidden || document.hidden) return;
    const current = ++revision;
    toggle.disabled = true;
    try {
      if (!api.getDebugSettings) throw new Error('Debug settings are unavailable in this browser build.');
      const state = await api.getDebugSettings();
      if (!stopped && current === revision) render(state);
    } catch (error) {
      if (!stopped && current === revision) {
        if (saved) render(saved);
        status.textContent = String(error);
      }
    }
  }

  async function save() {
    if (stopped || writing || !saved || saved.videoBufferWorkaroundForced || !api.setVideoBufferWorkaround) return;
    ++revision;
    writing = true;
    toggle.disabled = true;
    try {
      const state = await api.setVideoBufferWorkaround(toggle.checked);
      if (!stopped) render(state);
    } catch (error) {
      if (!stopped) {
        render(saved);
        status.textContent = `Could not save: ${String(error)}`;
      }
    } finally {
      writing = false;
      if (!stopped && saved) toggle.disabled = saved.videoBufferWorkaroundForced || !api.setVideoBufferWorkaround;
    }
  }

  const refreshVisible = () => void refresh();
  const change = () => void save();
  const observer = new MutationObserver(refreshVisible);
  observer.observe(pane, { attributes: true, attributeFilter: ['hidden'] });
  toggle.addEventListener('change', change);
  document.addEventListener('visibilitychange', refreshVisible);
  window.addEventListener('focus', refreshVisible);
  window.addEventListener('pagehide', () => {
    stopped = true;
    ++revision;
    observer.disconnect();
    toggle.removeEventListener('change', change);
    document.removeEventListener('visibilitychange', refreshVisible);
    window.removeEventListener('focus', refreshVisible);
  }, { once: true });
  refreshVisible();
}
