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
        let mut result = self.graph.execute(
            neo4rs::query(query), // use neo4rs string or query
        ).await.map_err(|e| e.to_string())?;

        // Simple stringification of result rows
        // Note: neo4rs returns rows as neo4rs::Row
        let mut output = String::new();
        /*
        while let Ok(Some(row)) = result.next().await {
            // we will need to format row roughly
            output.push_str("Row\n");
        }
        */
        Ok(format!("Executed query: {}", query)) // placeholder since neo4rs rows parsing is complex
    }
}
