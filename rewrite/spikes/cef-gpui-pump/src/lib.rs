//! The parts of the spike that need neither CEF nor GPUI, so `cargo test` can
//! run them without loading libcef.

pub mod schedule;
