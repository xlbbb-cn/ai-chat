//! Shared HTTP clients for upstream requests.
//!
//! One place owns the timeouts, so the model catalogue (`model_metadata`), the
//! chat stream (`llm_client`) and the sub-agents cannot drift apart.

use reqwest::{Client, ClientBuilder};
use std::time::Duration;

/// The app's identifying `User-Agent`: `rabbit-chat-studio/<version>`.
///
/// Lowercase ASCII by HTTP convention (the display name stays `Rabbit Chat
/// Studio`). The version is baked in from `Cargo.toml` at build time, so it
/// always matches the binary that sent the request — there is no runtime
/// "unknown version" case to fall back on. Used by the cloud-agent /
/// sub-agent clients ([`build_agent_client`]) and by the update check
/// (`crate::update`). The chat stream deliberately keeps reqwest's default
/// (no header): a gateway can only tell background agent traffic apart from
/// the conversation when the two differ.
pub(crate) const APP_USER_AGENT: &str = concat!("rabbit-chat-studio/", env!("CARGO_PKG_VERSION"));

/// Transport policy shared by the two streaming clients below: no total
/// request timeout (see [`build_stream_client`]), liveness per read, and
/// keep-alive / pool idle tuned for a desktop chat app.
fn stream_policy(builder: ClientBuilder) -> ClientBuilder {
    builder
        .connect_timeout(Duration::from_secs(12))
        .read_timeout(Duration::from_secs(120))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
}

/// Client for short, non-streaming requests (`/models`, catalogue fetches).
pub(crate) fn build_http_client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(12))
        .timeout(Duration::from_secs(300))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

/// Client for streaming chat requests.
///
/// Unlike [`build_http_client`] there is NO total request timeout: reqwest's
/// `timeout` covers the whole response body, which for an SSE stream is the
/// entire generation — a thinking model (or a slow relay) that streams longer
/// than the limit died mid-answer no matter how healthy the connection was.
/// Liveness is enforced per read instead: `read_timeout` fails when no bytes
/// arrive for the interval, so a stalled upstream errors quickly (and retries)
/// while a slow-but-alive stream may run arbitrarily long. Every caller that
/// runs `stream_llm_request` must use this builder.
pub(crate) fn build_stream_client() -> Result<Client, String> {
    stream_policy(Client::builder())
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

/// Client for sub-agent LLM calls (planner, executor, supervisor and their
/// capability checks).
///
/// Same transport policy as [`build_stream_client`] — sub-agent calls are
/// streamed too — plus [`APP_USER_AGENT`] so a provider or gateway sees who
/// sent the request instead of a nameless reqwest client.
pub(crate) fn build_agent_client() -> Result<Client, String> {
    stream_policy(Client::builder().user_agent(APP_USER_AGENT))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

#[cfg(test)]
mod tests {
    use super::APP_USER_AGENT;

    /// Lowercase ASCII and versioned: a gateway log line stays readable, and the
    /// version identifies the release that sent the request.
    #[test]
    fn app_user_agent_is_versioned_ascii() {
        assert_eq!(
            APP_USER_AGENT,
            format!("rabbit-chat-studio/{}", env!("CARGO_PKG_VERSION"))
        );
        assert!(APP_USER_AGENT.is_ascii());
        assert!(!APP_USER_AGENT.chars().any(|c| c.is_ascii_uppercase()));
    }
}
