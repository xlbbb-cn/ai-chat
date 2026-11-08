//! Knowledge graph clients — a standalone library crate (`knowledge-graph`).
//!
//! The client is deliberately **not** part of the tool set right now: nothing
//! advertises or dispatches a `knowledge_graph` tool, and `AppConfig` carries
//! no graph settings. Everything needed to (re)connect lives in this crate, so a
//! future integration only has to add a tool definition plus a dispatch arm that
//! builds [`Settings`] and calls [`Neo4jRepo::connect`].
//!
//! Layout:
//! - [`settings`] — engine id plus connection settings (backend agnostic).
//! - [`neo4j`] — the Neo4j implementation of [`KnowledgeGraph`].
//!
//! It is a workspace member of `src-tauri/Cargo.toml`, not a module of the app
//! crate: the app neither links `neo4rs` nor compiles this code until it adds
//! `knowledge-graph = { path = "src/utils/knowledge_graph" }` and wires a tool
//! back into `tools/mod.rs`. Everything is exercised by
//! `cargo test -p knowledge-graph`.

pub mod neo4j;
pub mod settings;

pub use neo4j::Neo4jRepo;
pub use settings::{Engine, Settings};

use async_trait::async_trait;

/// A queryable graph database.
///
/// Implementations own their connection; the caller only sees a query string
/// (Cypher for Neo4j) and a human-readable result.
#[async_trait]
pub trait KnowledgeGraph {
    /// Run `query` and return a short human-readable summary of the result.
    async fn execute_query(&self, query: &str) -> Result<String, String>;
}
