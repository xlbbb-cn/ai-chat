use reqwest::Client;
use scraper::{Html, Selector};
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
pub async fn search_duckduckgo(query: String) -> Result<String, String> {
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| e.to_string())?;

    // Primary: DuckDuckGo Instant Answer API (stable JSON, no key required)
    if let Ok(res) = client
        .get("https://api.duckduckgo.com/")
        .query(&[("q", query.as_str()), ("format", "json"), ("no_html", "1"), ("skip_disambig", "1")])
        .send()
        .await
    {
        if let Ok(json) = res.json::<serde_json::Value>().await {
            let mut results = String::new();

            if let Some(text) = json["AbstractText"].as_str() {
                if !text.is_empty() {
                    results.push_str(text);
                    results.push('\n');
                }
            }
            if let Some(answer) = json["Answer"].as_str() {
                if !answer.is_empty() {
                    results.push_str(answer);
                    results.push('\n');
                }
            }
            if let Some(topics) = json["RelatedTopics"].as_array() {
                for topic in topics.iter().take(4) {
                    if let Some(text) = topic["Text"].as_str() {
                        if !text.is_empty() {
                            results.push_str(text);
                            results.push('\n');
                        }
                    }
                }
            }

            if !results.is_empty() {
                return Ok(results);
            }
        }
    }

    // Fallback: DuckDuckGo HTML scraping
    let res = client
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query.as_str())])
        .header("Accept", "text/html,application/xhtml+xml")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let html = res.text().await.map_err(|e| e.to_string())?;
    let document = Html::parse_document(&html);

    let mut results = String::new();
    for sel_str in &[".result__snippet", ".result-snippet", "td.result-snippet", ".snippet"] {
        if let Ok(selector) = Selector::parse(sel_str) {
            for element in document.select(&selector).take(5) {
                let text = element.text().collect::<Vec<_>>().join(" ");
                let trimmed = text.trim().to_string();
                if !trimmed.is_empty() {
                    results.push_str(&trimmed);
                    results.push('\n');
                }
            }
        }
        if !results.is_empty() {
            break;
        }
    }

    if results.is_empty() {
        return Err("Web search returned no results. Try a more specific query.".to_string());
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
    let mut stmt = db.prepare(
        "SELECT id, session_id, role, content, COALESCE(timestamp, '') FROM history ORDER BY id ASC LIMIT 500"
    ).map_err(|e| e.to_string())?;
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
