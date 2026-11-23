//! Script Boosts as they run, from the shell's `features/compileBoost.ts` and `features/boosts.ts`.
//!
//! The shell compiles on the Boosts page with sucrase (`typescript` and `imports` transforms) and
//! wraps sucrase's output in a function. This crate holds the wrapping. The TypeScript transform
//! is the caller's: no Rust crate reproduces sucrase's output or its error text (see the README).

use serde::{Deserialize, Serialize};

use crate::name::BoostName;

/// A script Boost's build: the compiled code, or why it didn't compile.
///
/// An empty `code` runs nothing, as in the shell. A template that was never compiled has
/// neither field.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct CompiledBoost {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `compileBoost`: transform the TypeScript, then wrap the output for [`once_per_document`].
///
/// `transform` gets the Boost's name and source and returns JavaScript or an error message,
/// as sucrase does with `{ transforms: ['typescript', 'imports'], production: true }`.
pub fn compile_with(
    name: &BoostName,
    source: &str,
    transform: impl FnOnce(&str, &str) -> Result<String, String>,
) -> CompiledBoost {
    match transform(name.as_str(), source) {
        Ok(code) => CompiledBoost {
            code: Some(wrap_transformed(name, &code)),
            error: None,
        },
        Err(error) => CompiledBoost {
            code: None,
            error: Some(error),
        },
    }
}

/// The function `compileBoost` puts the transformed module in.
///
/// sucrase's `imports` transform writes to `exports`, so the wrapper declares it. The name is
/// safe in the `sourceURL` comment because a [`BoostName`] holds no line terminator.
///
/// The wrapper keeps the module's declarations out of the page's global scope. It isn't a
/// boundary against the Boost's own text: sucrase parses a module, where `<!--` is an operator,
/// and the page runs a classic script, where `<!--` starts a comment. Such a Boost can close
/// this function, and this guard, early. See `tests/parity.rs`.
pub fn wrap_transformed(name: &BoostName, code: &str) -> String {
    format!(
        "(() => {{\nconst exports = {{}}, module = {{ exports }};\n{code}\n}})();\n//# sourceURL=flecha-boost/{name}"
    )
}

/// `oncePerDocument`: what the shell injects, so a Boost runs once per document when both
/// injection paths reach it.
///
/// The marker is a registered symbol on the page's global object. The page and every other Boost
/// in the MAIN world can read it or set it first.
pub fn once_per_document(name: &BoostName, code: &str) -> String {
    let marker = marker(name);
    format!(
        "if (!Object.hasOwn(globalThis, {marker})) {{\n  Object.defineProperty(globalThis, {marker}, {{ value: true }});\n{code}\n}}"
    )
}

/// `Symbol.for(JSON.stringify("flecha.boost/<name>"))`
fn marker(name: &BoostName) -> String {
    let key =
        serde_json::to_string(&format!("flecha.boost/{name}")).expect("a string always serializes");
    format!("Symbol.for({key})")
}

/// The console line the shell shows for a Boost that didn't compile or failed to inject.
pub fn error_message(name: &BoostName, message: &str) -> String {
    format!("[Flecha boost {name}] {message}")
}

/// A classic script that logs `message` to the page's console, as `reportError` does.
pub fn console_error(message: &str) -> String {
    let message = serde_json::to_string(message).expect("a string always serializes");
    format!("console.error({message});")
}
