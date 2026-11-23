//! The MCP agent bridge: coding agents see and drive Flecha's tabs over loopback HTTP with a
//! bearer token. The server needs only a [`BrowserControl`]; [`fake::FakeBrowser`] serves tests.

mod control;
pub mod fake;
mod handler;
mod quiet;
mod server;
pub mod state;
mod token;
pub mod tools;

pub use control::*;
pub use handler::{PROTOCOL_CEILING, call, server_config, tool_list};
pub use server::{AgentServer, DEFAULT_AGENT_PORT};
pub use token::AgentToken;
