//! Opt-in, local-only MCP automation; all effects go through the window/session owner.
mod catalog;
mod control;
#[cfg(test)]
mod tests;
mod tools;
mod transport;

pub use transport::start;
pub use transport::{Config, Server, attach_stdio};

/// The exact tool catalog served by MCP, for both presentation modes.
pub fn schema() -> serde_json::Value {
    serde_json::json!({
        "tools": catalog::tools_for(true),
        "headless_tools": catalog::tools_for(false),
    })
}
