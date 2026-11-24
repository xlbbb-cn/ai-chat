//! The attach endpoint of the plugin message bus.
//!
//! A plugin does not have to be a child of this app: the Rust, Node or Python
//! service can run under its own supervisor and dial the bus. The listener is
//! loopback-only and every connection must present the workspace token in its
//! first line, so an unrelated local process cannot impersonate a plugin.
//!
//! Wire protocol (`rabbit-plugin-link/1`), one JSON object per line:
//!
//! ```text
//! client → host  {"type":"hello","plugin":"clock","token":"…","version":"1.0.0","instance":"clock-a"}
//! host → client  {"type":"welcome","plugin":"clock","instance":"clock#link-1","config":{…}}
//! each way       request / reply / event / log / ping / pong / config / shutdown
//! ```
//!
//! After `welcome` the connection is one instance of that plugin and takes
//! part in the same dispatch policy as a spawned one: N connections give N
//! instances, which is how a service scales itself without any manifest
//! change.

use serde::Serialize;
use std::io::ErrorKind;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpListener;
use tokio::sync::mpsc::UnboundedReceiver;

use super::{Sink, LOG_INFO, LOG_WARN};

/// Protocol tag carried in the hello exchange and echoed by the manager.
pub const LINK_PROTOCOL: &str = "rabbit-plugin-link/1";
/// Token file inside `<workspace>/.data/plugins/`.
pub const TOKEN_FILE: &str = ".link-token";
/// How long a connection may take to send its hello line.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// What a plugin needs to dial the bus; also shown in the manager window.
#[derive(Debug, Clone, Serialize)]
pub struct LinkEndpoint {
    pub protocol: String,
    pub host: String,
    pub port: u16,
    /// `host:port`, ready to paste into a client.
    pub address: String,
    /// Shared workspace secret; every attach must present it.
    pub token: String,
}

/// A connection that completed its hello and waits to become an instance.
pub struct Attachment {
    pub plugin_id: String,
    pub remote: String,
    pub reader: BufReader<OwnedReadHalf>,
    pub writer: OwnedWriteHalf,
    pub hello: serde_json::Value,
}

/// Load the workspace token, creating it on first use.
pub fn load_or_create_token(plugins_dir: &Path) -> Result<String, String> {
    let path = plugins_dir.join(TOKEN_FILE);
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if trimmed.len() >= 16 {
            return Ok(trimmed.to_string());
        }
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    std::fs::create_dir_all(plugins_dir)
        .map_err(|e| format!("create {}: {e}", plugins_dir.display()))?;
    std::fs::write(&path, &token).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(token)
}

/// Bind the loopback listener and start accepting attachments.
///
/// Returns the endpoint to hand to plugins plus the receiver the plugin host
/// drains; the host owns registration so this task never touches the instance
/// pool.
pub fn start(
    sink: Sink,
    token: String,
) -> Result<(LinkEndpoint, UnboundedReceiver<Attachment>), String> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Attachment>();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("bind plugin bus endpoint: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("plugin bus endpoint: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("plugin bus endpoint address: {e}"))?
        .port();
    let accept_token = token.clone();
    tokio::spawn(async move {
        let listener = match TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(err) => {
                sink.log("bus", LOG_WARN, format!("plugin bus endpoint unavailable: {err}"));
                return;
            }
        };
        sink.log(
            "bus",
            LOG_INFO,
            format!("plugin bus endpoint listening on 127.0.0.1:{port}"),
        );
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(pair) => pair,
                Err(err) => {
                    sink.log("bus", LOG_WARN, format!("plugin bus accept failed: {err}"));
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            };
            let _ = stream.set_nodelay(true);
            let sink = sink.clone();
            let token = accept_token.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut reader = BufReader::new(read);
                let mut line = String::new();
                let hello = tokio::time::timeout(HELLO_TIMEOUT, reader.read_line(&mut line)).await;
                let read_ok = matches!(hello, Ok(Ok(count)) if count > 0);
                if !read_ok {
                    let _ = write
                        .write_all(b"{\"type\":\"error\",\"error\":\"hello required\"}\n")
                        .await;
                    return;
                }
                let frame: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
                if frame["type"] != "hello" {
                    let _ = write
                        .write_all(b"{\"type\":\"error\",\"error\":\"hello required\"}\n")
                        .await;
                    return;
                }
                if frame["token"].as_str() != Some(token.as_str()) {
                    sink.log(
                        "bus",
                        LOG_WARN,
                        format!("plugin bus rejected {peer}: bad token"),
                    );
                    let _ = write
                        .write_all(b"{\"type\":\"error\",\"error\":\"bad token\"}\n")
                        .await;
                    return;
                }
                let plugin_id = frame["plugin"].as_str().unwrap_or("").trim().to_string();
                if plugin_id.is_empty() {
                    let _ = write
                        .write_all(b"{\"type\":\"error\",\"error\":\"plugin required\"}\n")
                        .await;
                    return;
                }
                sink.log(
                    &plugin_id,
                    LOG_INFO,
                    format!("service attached from {peer}"),
                );
                if tx
                    .send(Attachment {
                        plugin_id,
                        remote: peer.to_string(),
                        reader,
                        writer: write,
                        hello: frame,
                    })
                    .is_err()
                {
                    return;
                }
            });
        }
    });
    let endpoint = LinkEndpoint {
        protocol: LINK_PROTOCOL.to_string(),
        host: "127.0.0.1".to_string(),
        port,
        address: format!("127.0.0.1:{port}"),
        token,
    };
    Ok((endpoint, rx))
}

/// Tell a connection why it is not going to become an attached instance.
pub async fn reject(mut writer: OwnedWriteHalf, reason: &str) -> bool {
    let frame = serde_json::json!({ "type": "error", "error": reason }).to_string();
    match writer.write_all(format!("{frame}\n").as_bytes()).await {
        Ok(()) => {
            let _ = writer.shutdown().await;
            true
        }
        Err(err) => err.kind() == ErrorKind::BrokenPipe,
    }
}
