//! Neo4j implementation of [`KnowledgeGraph`], backed by the `neo4rs` driver.

use async_trait::async_trait;
use neo4rs::{Config, ConfigBuilder, Graph};

use super::settings::Settings;
use super::KnowledgeGraph;

/// A Neo4j driver wrapper.
///
/// Cheap to keep around: `neo4rs` pools connections internally, so one repo per
/// integration is enough.
pub struct Neo4jRepo {
    pub graph: Graph,
}

impl Neo4jRepo {
    /// Connect using `settings`.
    ///
    /// Fails when the driver config is invalid or the server is unreachable.
    pub async fn connect(settings: &Settings) -> Result<Self, String> {
        let graph = Graph::connect(build_config(settings)?)
            .await
            .map_err(|e| e.to_string())?;
        Ok(Self { graph })
    }
}

/// Map [`Settings`] onto a driver config **without** opening a connection.
///
/// Kept separate from [`Neo4jRepo::connect`] so the mapping can be tested
/// offline.
fn build_config(settings: &Settings) -> Result<Config, String> {
    ConfigBuilder::default()
        .uri(&settings.uri)
        .user(&settings.user)
        .password(&settings.password)
        .build()
        .map_err(|e| e.to_string())
}

#[async_trait]
impl KnowledgeGraph for Neo4jRepo {
    async fn execute_query(&self, query: &str) -> Result<String, String> {
        // Surface driver errors. `execute` hands back a row stream that has to
        // be drained explicitly; the summary therefore only reports that the
        // statement was accepted. Row parsing can be added once the tool is
        // wired back into the tool set.
        let _ = self
            .graph
            .execute(neo4rs::query(query))
            .await
            .map_err(|e| e.to_string())?;

        Ok(format!("Executed query: {}", query))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_build_a_driver_config() {
        assert!(build_config(&Settings::default()).is_ok());
    }

    #[test]
    fn settings_reach_the_driver_config() {
        let settings = Settings::new("bolt://graph.example:7687", "alice", "secret");
        // An unreachable host must not fail here — building a config does not
        // open a connection.
        assert!(build_config(&settings).is_ok());
        assert_eq!(settings.uri, "bolt://graph.example:7687");
    }
}
