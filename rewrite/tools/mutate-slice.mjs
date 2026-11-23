import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';

const [mutation] = process.argv.slice(2);
const engine = 'crates/flecha-cef/src/lib.rs';

const mutations = {
  cookie: [engine, 'if *partition == Partition::Shared {', 'if *partition == Partition::Shared || matches!(partition, Partition::Isolated(_)) {'],
  stale: [engine, 'self.generations.accepts(&message.key)', 'self.generations.current(&message.key.tab).is_some()'],
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
