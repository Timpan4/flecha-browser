import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';

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
  'blank-restart': [`${ui}/view.rs`, '.child(surface(frames.view, frames.popup))', '.child(surface(if std::path::Path::new("/mnt/scratch/restart.log").exists() { None } else { frames.view }, frames.popup))'],
  'close-archive': [`${ui}/lib.rs`, 'self.engine.close(&tab);\n                    for removed in self.model.core.close_tab', 'self.engine.close(&tab);\n                    self.model.core.tab_mut(&tab).unwrap().url = "about:blank".into();\n                    for removed in self.model.core.close_tab'],
  'archive-dup': ['crates/flecha-core/src/state.rs', 'let result = self.close_tab_recording(id, now, |id| removed.push(id));', 'let result = self.close_tab_recording(id, now, |id| removed.push(id));\n        if self.archive.len() > 1 { self.archive[0] = self.archive[1].clone(); }'],
  'stale-openlink': [engine, 'if !self.generations.accepts(&message.key) {', 'if !self.generations.accepts(&message.key) && !matches!(message.event, Event::OpenLink { .. } | Event::Loaded { .. } | Event::PaintFailed) {'],
  'peek-shared': [`${ui}/model.rs`, '.unwrap_or_else(|| self.space_partition(active_space))', '.unwrap_or_else(|| { let _ = active_space; Partition::Shared })'],
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
