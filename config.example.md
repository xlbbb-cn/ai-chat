# config.json Example

This project stores runtime configuration in app data as config.json.

Typical locations:
- Windows: %APPDATA%/../Local/ai-chat/config.json (depends on Tauri app data path)
- macOS: ~/Library/Application Support/ai-chat/config.json
- Linux: ~/.local/share/ai-chat/config.json

## Example

```json
{
  "api_base_url": "https://api.openai.com/v1",
  "api_key": "sk-...",
  "model": "gpt-4o-mini",
  "temperature": 0.7,
  "reasoning_effort": "medium",
  "system_message": "You are a helpful assistant.",
  "selected_tools": ["web_search", "file_actions", "knowledge_graph"],
  "search_engine": "duckduckgo",
  "kg_engine": "neo4j",
  "neo4j_uri": "bolt://localhost:7687",
  "neo4j_user": "neo4j",
  "neo4j_password": "your_password"
}
```

## Field Notes

- api_base_url: OpenAI-compatible API base URL.
- api_key: API key for the model provider.
- model: Model name used for chat completion.
- temperature: Sampling temperature. Higher means more diverse output.
- reasoning_effort: Reasoning level hint used by some models.
- system_message: Global system prompt appended to each conversation.
- selected_tools: Enabled tools list. Supported values include web_search, execute_command, fetch_web, file_actions, knowledge_graph.
- search_engine: Default engine used by web_search.
- kg_engine: Knowledge graph backend selector. Current default is neo4j.
- neo4j_uri: Neo4j Bolt endpoint, for example bolt://localhost:7687.
- neo4j_user: Neo4j username.
- neo4j_password: Neo4j password.

## Security

- Do not commit real keys or passwords.
- Prefer environment-specific config files for production deployment.
