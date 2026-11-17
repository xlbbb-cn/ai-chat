//! Shared HTTP clients for upstream requests.
//!
//! One place owns the timeouts, so the model catalogue (`model_metadata`), the
//! chat stream (`llm_client`) and the sub-agents cannot drift apart.

use reqwest::Client;
use std::time::Duration;

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
    Client::builder()
        .connect_timeout(Duration::from_secs(12))
        .read_timeout(Duration::from_secs(120))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}
