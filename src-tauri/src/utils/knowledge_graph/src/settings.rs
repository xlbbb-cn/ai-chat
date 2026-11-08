//! Engine selection and connection settings.
//!
//! The settings are owned by this module instead of `AppConfig`: credentials
//! never reach `config.json`, and the caller decides where the values come
//! from (env vars, a future config section, a test fixture).

/// Backends this module can talk to.
///
/// Only Neo4j exists today; keeping an enum makes the call sites
/// engine-agnostic, so a second backend is a new variant plus a new module.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Engine {
    /// Neo4j over Bolt (`neo4rs` driver).
    #[default]
    Neo4j,
}

impl Engine {
    /// Every supported engine, in display order.
    pub const ALL: &'static [Engine] = &[Engine::Neo4j];

    /// Parse an engine id (`"neo4j"`).
    ///
    /// Returns `None` for anything unknown so a stale or misspelled value
    /// cannot silently select a different backend.
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim() {
            "neo4j" => Some(Engine::Neo4j),
            _ => None,
        }
    }

    /// The stable id used in config files and UI payloads.
    pub fn id(self) -> &'static str {
        match self {
            Engine::Neo4j => "neo4j",
        }
    }
}

/// Connection settings for one engine.
///
/// The defaults describe a stock local Neo4j (`bolt://localhost:7687`, user
/// `neo4j`, empty password) and match what the app used while the graph was
/// still configured through `AppConfig`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub engine: Engine,
    pub uri: String,
    pub user: String,
    pub password: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            engine: Engine::Neo4j,
            uri: "bolt://localhost:7687".to_string(),
            user: "neo4j".to_string(),
            password: String::new(),
        }
    }
}

impl Settings {
    /// Settings for the default engine with an explicit endpoint / credential.
    pub fn new(
        uri: impl Into<String>,
        user: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            uri: uri.into(),
            user: user.into(),
            password: password.into(),
            ..Self::default()
        }
    }

    /// Switch the engine, keeping the rest of the settings.
    pub fn with_engine(mut self, engine: Engine) -> Self {
        self.engine = engine;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_targets_a_local_neo4j() {
        let settings = Settings::default();
        assert_eq!(settings.engine, Engine::Neo4j);
        assert_eq!(settings.uri, "bolt://localhost:7687");
        assert_eq!(settings.user, "neo4j");
        assert!(settings.password.is_empty());
    }

    #[test]
    fn engine_ids_round_trip_and_reject_unknown_values() {
        assert_eq!(Engine::from_id("neo4j"), Some(Engine::Neo4j));
        assert_eq!(Engine::from_id(" neo4j "), Some(Engine::Neo4j));
        assert_eq!(Engine::from_id(Engine::Neo4j.id()), Some(Engine::Neo4j));
        assert_eq!(Engine::from_id("memgraph"), None);
        assert_eq!(Engine::from_id(""), None);
    }

    #[test]
    fn new_keeps_the_engine_and_overrides_the_endpoint() {
        let settings = Settings::new("bolt://graph.example:7687", "alice", "secret");
        assert_eq!(settings.engine, Engine::Neo4j);
        assert_eq!(settings.uri, "bolt://graph.example:7687");
        assert_eq!(settings.user, "alice");
        assert_eq!(settings.password, "secret");
    }
}
