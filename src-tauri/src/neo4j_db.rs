use neo4rs::{Graph, ConfigBuilder};
use async_trait::async_trait;

#[async_trait]
pub trait KnowledgeGraph {
    async fn execute_query(&self, query: &str) -> Result<String, String>;
}

pub struct Neo4jRepo {
    pub graph: Graph,
}

impl Neo4jRepo {
    pub async fn new(uri: &str, user: &str, pass: &str) -> Result<Self, String> {
        let config = ConfigBuilder::default()
            .uri(uri)
            .user(user)
            .password(pass)
            .build()
            .map_err(|e| e.to_string())?;

        let graph = Graph::connect(config)
            .await
            .map_err(|e| e.to_string())?;

        Ok(Self { graph })
    }
}

#[async_trait]
impl KnowledgeGraph for Neo4jRepo {
    async fn execute_query(&self, query: &str) -> Result<String, String> {
        // Execute the query and surface driver errors; detailed row parsing can be added later.
        let _ = self
            .graph
            .execute(neo4rs::query(query))
            .await
            .map_err(|e| e.to_string())?;

        Ok(format!("Executed query: {}", query))
    }
}
