use serde::Serialize;

#[derive(Serialize)]
pub struct HistoryRecord {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

#[tauri::command]
pub fn save_history(
    session_id: String,
    role: String,
    content: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute(
        "INSERT INTO history (session_id, role, content) VALUES (?1, ?2, ?3)",
        rusqlite::params![session_id, role, content],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn load_history(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<HistoryRecord>, String> {
    let db = state.db.lock().unwrap();
    let mut stmt = db
        .prepare(
            "SELECT id, session_id, role, content, COALESCE(timestamp, '') \
             FROM history ORDER BY id ASC LIMIT 500",
        )
        .map_err(|e| e.to_string())?;
    let history = stmt
        .query_map([], |row| {
            Ok(HistoryRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                timestamp: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    Ok(history)
}
