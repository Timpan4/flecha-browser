/**
 * The Settings page in the Chromium shell: Electron's Settings renderer as is, with
 * `window.settings` provided over the hub. Imports run in order, so the API is in place
 * before src/renderer/settings.ts reads it.
 */
import './settingsApi';
import '../../src/renderer/settings';
