import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';

import { blankRestartColor } from './fixture.mjs';

const [mutation] = process.argv.slice(2);
const engine = 'crates/flecha-cef/src/lib.rs';
const ui = 'crates/flecha-ui/src';

const mutations = {
  cookie: [engine, 'if *partition == Partition::Shared {', 'if *partition == Partition::Shared || matches!(partition, Partition::Isolated(_)) {'],
  stale: [engine, 'self.generations.accepts(&message.key)', 'self.generations.current(&message.key.tab).is_some()'],
  'stale-paint': [engine, 'if !self.generations.accepts(&message.key) {', 'if !self.generations.accepts(&message.key) && message.event != Event::Paint {'],
  'stale-title': [engine, 'if !self.generations.accepts(&message.key) {', 'if !self.generations.accepts(&message.key) && matches!(message.event, Event::Created(_)) {'],
  'split-swap': [`${ui}/view.rs`, 'for (index, tab) in view.iter().enumerate() {', 'for (index, tab) in view.iter().rev().enumerate() {'],
  'pane-size': [`${ui}/view.rs`, '} else if index == 0 {\n                    ratio\n', '} else if index == 0 {\n                    ratio * 0.5\n'],
  'pane-height': [`${ui}/view.rs`, 'pages = pages.child(div().h_full().w(relative(width)).child(pane));', 'pages = pages.child(div().h(relative(if index == 1 { 0.3 } else { 1.0 })).w(relative(width)).child(pane));'],
  'pane-offset': [`${ui}/view.rs`, 'pages = pages.child(div().h_full().w(relative(width)).child(pane));', 'pages = pages.child(div().h_full().w(relative(width)).when(index == 0, |d| d.ml(px(3.))).child(pane));'],
  'ratio-002': [`${ui}/view.rs`, '.unwrap_or(0.5) as f32;', '.unwrap_or(0.5) as f32 + 0.002;'],
  'ratio-001': [`${ui}/view.rs`, '.unwrap_or(0.5) as f32;', '.unwrap_or(0.5) as f32 + 0.001;'],
  'right-both': [`${ui}/view.rs`, 'state.engine.frames(&id).unwrap_or_default(),',
    '{ let pick = state.model.core.window(&self.id).and_then(|w| w.spaces.get(&w.active_space_id)).and_then(|s| s.split.clone()).filter(|s| s[0] == id).map(|s| s[1].clone()).unwrap_or_else(|| id.clone()); state.engine.frames(&pick).unwrap_or_default() },'],
  'pane-size-restart': [`${ui}/view.rs`, '} else if index == 0 {\n                    ratio\n',
    '} else if index == 0 {\n                    if std::path::Path::new("/mnt/scratch/restart.log").exists() { ratio * 0.5 } else { ratio }\n'],
  'blank-restart': [`${ui}/view.rs`, '.child(surface(frames.view, frames.popup))', `
            .when(std::path::Path::new("/mnt/scratch/restart.log").exists(), |pane| pane.bg(rgb(0x${blankRestartColor.slice(1)})))
            .child(surface(if std::path::Path::new("/mnt/scratch/restart.log").exists() { None } else { frames.view }, frames.popup))`],
  'close-archive': [`${ui}/sidebar.rs`, '        self.engine.close(tab);\n        for removed in self.model.close_tab(tab) {', '        self.engine.close(tab);\n        self.model.core.tab_mut(tab).unwrap().url = "about:blank".into();\n        for removed in self.model.close_tab(tab) {'],
  'stale-closedat-ui': [`${ui}/model.rs`, '.map(|(state, _)| state.close_tab(id, now()).removed)', '.map(|(state, _)| { let stale = state.tab(id).and_then(|t| t.last_active).map_or(now(), |at| at as flecha_core::EpochMillis); state.close_tab(id, stale).removed })'],
  'stale-closedat': ['crates/flecha-core/src/state.rs', '        let result = self.close_tab_recording(id, now, |id| removed.push(id));\n',
    '        let stale = self.tabs.get(id).and_then(|tab| tab.last_active).map_or(now, |at| at as EpochMillis);\n        let result = self.close_tab_recording(id, stale, |id| removed.push(id));\n'],
  'archive-dup': ['crates/flecha-core/src/state.rs', 'let result = self.close_tab_recording(id, now, |id| removed.push(id));', 'let result = self.close_tab_recording(id, now, |id| removed.push(id));\n        if self.archive.len() > 1 { self.archive[0] = self.archive[1].clone(); }'],
  'stale-opened': [engine, 'if !self.generations.accepts(&message.key) {', 'if !self.generations.accepts(&message.key) && !matches!(message.event, Event::Opened(_) | Event::ClosedByPage | Event::Loaded { .. } | Event::PaintFailed) {'],
  'new-variant': [engine, '    ClosedByPage,\n}', '    ClosedByPage,\n    Favicon(String),\n}'],
  'popups-allowed': [engine, '        let key = self.generations.create(tab.clone());\n',
    '        cef::ImplRequestContext::set_content_setting(&context, None, None, cef::ContentSettingTypes::POPUPS, cef::ContentSettingValues::ALLOW);\n        let key = self.generations.create(tab.clone());\n'],
  'popup-keyboard': [`${ui}/model.rs`, '        if let Some(popup) = self.popups.get(window) {\n            return Some((Owner::Popup(window.clone()), &popup.page.id));\n        }\n', ''],
  'peek-shared': [`${ui}/model.rs`, '.unwrap_or_else(|| self.window_partition(&window, &space))', '.unwrap_or(Partition::Shared)'],
  'peek-blind-click': [`${ui}/view.rs`, 'if state.model.peeks.contains_key(&self.id) {\n            main = main.child(', 'if false {\n            main = main.child('],
  'flechinha-old-partition': [`${ui}/model.rs`, '.extract_if(.., |_, page| page.from_regular_space(space)),', '.extract_if(.., |_, _| false),'],
  timeout: ['app/src/main.rs', 'fn main() -> Result<()> {', `
fn _forbidden_timeout(executor: &gpui::BackgroundExecutor) {
    use gpui::FutureExt as _;
    let _pending = std::future::pending::<()>().with_timeout(std::time::Duration::ZERO, executor);
}

fn main() -> Result<()> {`],
};

const [path, original, replacement] = mutations[mutation];
const bytes = readFileSync(path, 'utf8');

assert.equal(bytes.split(original).length, 2, `one call site must match the ${mutation} mutation`);

writeFileSync(path, bytes.replace(original, replacement));
