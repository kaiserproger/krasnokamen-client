//! Opt-in, local-only MCP automation; all effects go through the window/session owner.
mod catalog;
mod control;
#[cfg(test)]
mod tests;
mod tools;
mod transport;

pub use transport::start;
pub use transport::{Config, Server, attach_stdio};

/// Export the same native schemas used by the live MCP transport.
pub(crate) fn schema() -> serde_json::Value {
    serde_json::json!({
        "tools": catalog::tools(),
        "headless_tools": catalog::tools().iter()
            .filter(|tool| tool["name"].as_str().is_some_and(catalog::state_supported))
            .collect::<Vec<_>>(),
    })
}
