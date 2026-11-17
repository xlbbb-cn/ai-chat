//! Commands for `workspace` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use crate::pdf_mentions::PdfMention;
use crate::pdf_mentions::PdfMentionProgress;
use crate::workspace_files::ImportedPaths;
use crate::workspace_files::WorkspaceFileEntry;
use crate::workspace_files::WorkspaceFileRange;
use crate::AppState;
use crate::WorkspaceHistoryEntry;
use tauri::ipc::Channel;
use tauri::AppHandle;
use tauri::State;

#[tauri::command]
pub(crate) async fn list_workspace_files(
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceFileEntry>, String> {
    crate::workspace_files::list_workspace_files(state).await
}

#[tauri::command]
pub(crate) async fn read_workspace_file_range(
    state: State<'_, AppState>,
    path: String,
    start_line: Option<u64>,
    end_line: Option<u64>,
) -> Result<WorkspaceFileRange, String> {
    crate::workspace_files::read_workspace_file_range(state, path, start_line, end_line).await
}

#[tauri::command]
pub(crate) async fn read_workspace_file_bytes(
    state: State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    crate::workspace_files::read_workspace_file_bytes(state, path).await
}

#[tauri::command]
pub(crate) async fn save_pasted_file(
    state: State<'_, AppState>,
    name: String,
    data_base64: String,
) -> Result<String, String> {
    crate::workspace_files::save_pasted_file(state, name, data_base64).await
}

#[tauri::command]
pub(crate) async fn import_workspace_paths(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<ImportedPaths, String> {
    crate::workspace_files::import_workspace_paths(state, paths).await
}

#[tauri::command]
pub(crate) async fn convert_pdf_mention(
    app: AppHandle,
    path: String,
    start_page: Option<usize>,
    end_page: Option<usize>,
    on_progress: Channel<PdfMentionProgress>,
) -> Result<PdfMention, String> {
    crate::pdf_mentions::convert_pdf_mention(app, path, start_page, end_page, on_progress).await
}

#[tauri::command]
pub(crate) fn get_workspace_dir(state: State<'_, AppState>) -> String {
    crate::get_workspace_dir(state)
}

#[tauri::command]
pub(crate) fn set_workspace_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    workspace_dir: Option<String>,
) -> Result<String, String> {
    crate::set_workspace_dir(app, state, workspace_dir)
}

#[tauri::command]
pub(crate) fn workspace_history_entries(state: State<'_, AppState>) -> Vec<WorkspaceHistoryEntry> {
    crate::workspace_history_entries(state)
}
