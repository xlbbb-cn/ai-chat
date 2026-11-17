//! Commands for `agents` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use crate::agents::cli;
use crate::agents::cloud;
use crate::agents::exchange;
use crate::agents::mbus::AgentBusMessageView;
use crate::agents::AgentMention;
use crate::agents::AgentOrchestration;
use crate::agents::KeyValue;
use crate::agents::MissionStateView;
use crate::agents::SubAgent;
use crate::AppState;
use tauri::State;

#[tauri::command]
pub(crate) fn list_sub_agents(state: State<'_, AppState>) -> Vec<SubAgent> {
    crate::agents::list_sub_agents(state)
}

#[tauri::command]
pub(crate) fn list_agent_mentions(state: State<'_, AppState>) -> Vec<AgentMention> {
    crate::agents::list_agent_mentions(state)
}

#[tauri::command]
pub(crate) fn refresh_agent_index(state: State<'_, AppState>) -> Result<Vec<SubAgent>, String> {
    crate::agents::refresh_agent_index(state)
}

#[tauri::command]
pub(crate) fn save_sub_agent(
    state: State<'_, AppState>,
    agent: SubAgent,
) -> Result<SubAgent, String> {
    crate::agents::save_sub_agent(state, agent)
}

#[tauri::command]
pub(crate) fn delete_sub_agent(state: State<'_, AppState>, id: String) -> Result<(), String> {
    crate::agents::delete_sub_agent(state, id)
}

#[tauri::command]
pub(crate) fn get_agent_orchestration(state: State<'_, AppState>) -> AgentOrchestration {
    crate::agents::get_agent_orchestration(state)
}

#[tauri::command]
pub(crate) fn save_agent_orchestration(
    state: State<'_, AppState>,
    orchestration: AgentOrchestration,
) -> Result<(), String> {
    crate::agents::save_agent_orchestration(state, orchestration)
}

#[tauri::command]
pub(crate) async fn probe_agent_cli(
    command: String,
    args: Option<Vec<String>>,
) -> Result<cli::CliProbe, String> {
    crate::agents::probe_agent_cli(command, args).await
}

#[tauri::command]
pub(crate) async fn probe_agent_cloud(
    endpoint: String,
    token: Option<String>,
    headers: Option<Vec<KeyValue>>,
) -> Result<cloud::CloudProbe, String> {
    crate::agents::probe_agent_cloud(endpoint, token, headers).await
}

#[tauri::command]
pub(crate) fn list_deliverables(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<exchange::DeliverableBatch>, String> {
    crate::agents::list_deliverables(state, session_id)
}

#[tauri::command]
pub(crate) fn list_agent_missions(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<MissionStateView>, String> {
    crate::agents::list_agent_missions(state, session_id)
}

#[tauri::command]
pub(crate) fn list_mission_bus_messages(
    state: tauri::State<'_, AppState>,
    mission_id: String,
    limit: Option<usize>,
) -> Result<Vec<AgentBusMessageView>, String> {
    crate::agents::mbus::list_mission_bus_messages(state, mission_id, limit)
}
