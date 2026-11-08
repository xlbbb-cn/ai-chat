//! Knowledge graph clients — a self-contained module library.
//!
//! The client is deliberately **not** part of the tool set right now: nothing
//! advertises or dispatches a `knowledge_graph` tool, and `AppConfig` carries
//! no graph settings. Everything needed to (re)connect lives in this directory,
//! so a future integration only has to add a tool definition plus a dispatch
//! arm that builds [`Settings`] and calls [`Neo4jRepo::connect`].
//!
//! Layout:
//! - [`settings`] — engine id plus connection settings (backend agnostic).
//! - [`neo4j`] — the Neo4j implementation of [`KnowledgeGraph`].
//!
//! The module is registered from the crate root as
//! `#[path = "utils/knowledge_graph/mod.rs"] pub mod knowledge_graph;`, which is
//! why there is no `utils/mod.rs`: `src/utils/` stays a plain folder on disk
//! (it also holds the `pdf2md-rs` crate) instead of becoming a module of its
//! own.

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
