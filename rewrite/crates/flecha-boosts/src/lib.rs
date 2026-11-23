//! Boosts: per-site user CSS and TypeScript, ported from the Chromium shell.
//!
//! The shell's behaviour is the reference: `chromium/shell/lib/boostFiles.ts` (names and
//! matching), `features/boosts.ts` (storage and injection) and `features/compileBoost.ts`
//! (the compiled wrapper). `tests/parity.rs` replays fixtures recorded from that code.

pub mod compile;
mod matching;
mod name;
mod shell;
pub mod snapshot;
mod store;

pub use compile::CompiledBoost;
pub use matching::{applies, boost_host_for, boost_matches, user_script_matches};
pub use name::{BoostKind, BoostName};
pub use shell::{SHELL_BUILDS_KEY, SHELL_FILES_KEY, ShellImport};
pub use store::{BoostFile, Boosts, Effect, Frame, RunAt, ScriptInjection, StyleInjection};
