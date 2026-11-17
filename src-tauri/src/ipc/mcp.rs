//! Commands for `mcp` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use crate::mcp::McpLogEntry;
use crate::mcp::McpServer;
use crate::AppState;
use tauri::State;

#[tauri::command]
pub(crate) fn list_mcp_servers(state: State<'_, AppState>) -> Vec<McpServer> {
    crate::mcp::list_mcp_servers(state)
}

#[tauri::command]
pub(crate) fn save_mcp_server(state: State<'_, AppState>, server: McpServer) -> Result<(), String> {
    crate::mcp::save_mcp_server(state, server)
}

#[tauri::command]
pub(crate) fn delete_mcp_server(state: State<'_, AppState>, id: String) -> Result<(), String> {
    crate::mcp::delete_mcp_server(state, id)
}

#[tauri::command]
pub(crate) async fn mcp_start_stdio(
    state: State<'_, AppState>,
    server_id: String,
) -> Result<(), String> {
    crate::mcp::mcp_start_stdio(state, server_id).await
}

#[tauri::command]
pub(crate) async fn mcp_stop_stdio(
    state: State<'_, AppState>,
    server_id: String,
) -> Result<(), String> {
    crate::mcp::mcp_stop_stdio(state, server_id).await
}

#[tauri::command]
pub(crate) fn get_mcp_logs(state: State<'_, AppState>, id: String) -> Vec<McpLogEntry> {
    crate::mcp::get_mcp_logs(state, id)
}

#[tauri::command]
pub(crate) fn clear_mcp_logs(state: State<'_, AppState>, id: String) {
    crate::mcp::clear_mcp_logs(state, id)
}

#[tauri::command]
pub(crate) async fn test_mcp_server(
    state: State<'_, AppState>,
    server: McpServer,
) -> Result<String, String> {
    crate::mcp::test_mcp_server(state, server).await
}
