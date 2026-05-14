use reqwest::Client;
use rusqlite::Connection;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct HistoryRecord {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: String,
}

#[tauri::command]
pub async fn search_duckduckgo(query: String) -> Result<String, String> {
    let url = format!("https://html.duckduckgo.com/html/?q={}", query);
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .build()
        .map_err(|e| e.to_string())?;
    
    let res = client.get(&url).send().await.map_err(|e| e.to_string())?;
    let html = res.text().await.map_err(|e| e.to_string())?;
    
    let document = Html::parse_document(&html);
    let selector = Selector::parse(".result__snippet").unwrap();
    
    let mut results = String::new();
    for element in document.select(&selector).take(3) {
        let text = element.text().collect::<Vec<_>>().join("");
        results.push_str(&text.trim());
        results.push_str("\n\n");
    }
    
    Ok(results)
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
    ).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn load_history(state: tauri::State<'_, crate::AppState>) -> Result<Vec<HistoryRecord>, String> {
    let db = state.db.lock().unwrap();
    let mut stmt = db.prepare("SELECT id, session_id, role, content FROM history ORDER BY id DESC LIMIT 50").map_err(|e| e.to_string())?;
    let history = stmt
        .query_map([], |row| {
            Ok(HistoryRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
        
    Ok(history)
}
