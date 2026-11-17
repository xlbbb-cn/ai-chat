//! IPC command layer.
//!
//! Every Tauri command lives here and does nothing but forward to the owning
//! domain module. The command names are the frontend contract (`src/api.ts`);
//! the implementations stay next to their business logic.

pub(crate) mod agents;
pub(crate) mod app;
pub(crate) mod chat;
pub(crate) mod config;
pub(crate) mod evolution;
pub(crate) mod experiences;
pub(crate) mod figures;
pub(crate) mod history;
pub(crate) mod mcp;
pub(crate) mod notify;
pub(crate) mod skills;
pub(crate) mod tools;
pub(crate) mod update;
pub(crate) mod workspace;
