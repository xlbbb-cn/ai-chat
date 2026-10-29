// Execution backend for `cloud`-type sub-agents: sends the task to a remote
// HTTP agent endpoint and returns its result. Supports both a single
// request/response endpoint and an asynchronous job that is polled until it
// reports completion.
//
// Everything protocol-specific is configurable on the agent (request body
// template, result/status dot paths, polling URL), so this works with an
// OpenAI-compatible chat endpoint as well as custom agent services.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use super::{resolve_agent_timeout, KeyValue, SubAgent};

/// Timeout for the panel's connectivity probe.
const PROBE_TIMEOUT_SECS: u64 = 30;
/// Longest a single HTTP call may take.
const REQUEST_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Clone, Serialize)]
pub struct CloudProbe {
    pub endpoint: String,
    pub reachable: bool,
    pub status: Option<u16>,
    pub detail: String,
}

pub struct CloudRunOutcome {
    pub result: String,
    pub duration_ms: u64,
    pub job_id: Option<String>,
    pub summary: String,
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Escapes a value for inclusion *inside* a JSON string literal (the template
/// supplies the surrounding quotes).
fn json_escape_fragment(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

fn expand_body(template: &str, prompt: &str, agent_name: &str, cwd: &Path) -> String {
    template
        .replace("{prompt}", &json_escape_fragment(prompt))
        .replace("{agent_name}", &json_escape_fragment(agent_name))
        .replace("{cwd}", &json_escape_fragment(&cwd.to_string_lossy()))
}

/// Reads a dot path (`choices.0.message.content`) out of a JSON value.
fn json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Some(value);
    }
    let mut current = value;
    for part in trimmed.split('.') {
        if part.is_empty() {
            continue;
        }
        current = match current {
            Value::Object(map) => map.get(part)?,
            Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

fn extract_text(value: &Value, path: &str) -> Result<String, String> {
    json_path(value, path)
        .map(value_to_text)
        .ok_or_else(|| format!("the response has no '{}' field", path.trim()))
}

fn emit_progress(app: &AppHandle, task_id: &str, text: String) {
    let _ = app.emit("agent-task-token", json!({ "task_id": task_id, "token": text }));
}

fn apply_headers(
    mut request: reqwest::RequestBuilder,
    token: &str,
    headers: &[KeyValue],
) -> reqwest::RequestBuilder {
    if !token.trim().is_empty() {
        request = request.bearer_auth(token.trim());
    }
    for KeyValue { key, value } in headers {
        if !key.trim().is_empty() {
            request = request.header(key.trim(), value.trim());
        }
    }
    request
}

fn status_matches(status: &str, values: &[String]) -> bool {
    values
        .iter()
        .any(|value| value.trim().eq_ignore_ascii_case(status.trim()))
}

// ─── Probe ───────────────────────────────────────────────────────────────────

/// Best-effort reachability check: GET the endpoint with a short timeout. Any
/// HTTP status counts as reachable — the point is to catch typos, DNS failures
/// and TLS/connection problems before a run.
pub async fn probe(
    endpoint: &str,
    token: &str,
    headers: &[KeyValue],
) -> Result<CloudProbe, String> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() {
        return Err("The endpoint URL is empty.".to_string());
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err("The endpoint must start with http:// or https://".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;

    let request = apply_headers(client.get(trimmed), token, headers);
    match request.send().await {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(300).collect();
            Ok(CloudProbe {
                endpoint: trimmed.to_string(),
                reachable: true,
                status: Some(status.as_u16()),
                detail: if snippet.trim().is_empty() {
                    format!("HTTP {status}")
                } else {
                    format!("HTTP {status}\n{snippet}")
                },
            })
        }
        Err(err) => Ok(CloudProbe {
            endpoint: trimmed.to_string(),
            reachable: false,
            status: None,
            detail: err.to_string(),
        }),
    }
}

// ─── Execution ───────────────────────────────────────────────────────────────

/// Runs a `cloud`-type agent: POSTs the task, then either reads the result
/// straight from the response or polls `poll_url` until it is done.
pub async fn run(
    app: &AppHandle,
    agent: &SubAgent,
    task_id: &str,
    prompt: &str,
    cwd: &Path,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<CloudRunOutcome, String> {
    let settings = &agent.cloud;
    let endpoint = settings.endpoint.trim();
    if endpoint.is_empty() {
        return Err("this cloud agent has no endpoint configured".to_string());
    }
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        return Err("the endpoint must start with http:// or https://".to_string());
    }

    let overall_timeout = resolve_agent_timeout(settings.timeout_seconds);
    let started = std::time::Instant::now();
    let deadline = overall_timeout.map(|timeout| tokio::time::Instant::now() + timeout);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;

    let body_text = expand_body(&settings.body_template, prompt, &agent.name, cwd);
    // Fail fast on a broken template instead of shipping garbage upstream.
    let body: Value = serde_json::from_str(&body_text)
        .map_err(|e| format!("the request body template is not valid JSON: {e}"))?;

    emit_progress(app, task_id, format!("POST {endpoint}\n"));
    let request = apply_headers(client.post(endpoint), &settings.token, &settings.headers)
        .header("Content-Type", "application/json")
        .json(&body);

    let response = request
        .send()
        .await
        .map_err(|e| format!("request to {endpoint} failed: {e}"))?;
    let status = response.status();
    let payload_text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let snippet: String = payload_text.chars().take(600).collect();
        return Err(format!("{endpoint} returned HTTP {status}: {snippet}"));
    }

    let payload: Value = serde_json::from_str(&payload_text)
        .map_err(|e| format!("the response is not valid JSON: {e}"))?;

    // ── Single-shot mode ────────────────────────────────────────────────────
    if settings.poll_url.trim().is_empty() {
        let result = extract_text(&payload, &settings.result_path)?;
        emit_progress(app, task_id, format!("HTTP {status} (no polling configured)\n"));
        return Ok(CloudRunOutcome {
            result,
            duration_ms: started.elapsed().as_millis() as u64,
            job_id: None,
            summary: format!("cloud agent \"{}\" answered directly (HTTP {status})", agent.name),
        });
    }

    // ── Async mode: poll until done ─────────────────────────────────────────
    let job_id = json_path(&payload, &settings.poll_id_path)
        .map(value_to_text)
        .unwrap_or_default();
    if job_id.trim().is_empty() {
        return Err(format!(
            "the response has no job id at '{}'",
            settings.poll_id_path.trim()
        ));
    }
    let poll_url = settings.poll_url.replace("{id}", job_id.trim());
    emit_progress(
        app,
        task_id,
        format!("HTTP {status} accepted, job {}\nPolling {poll_url}\n", job_id.trim()),
    );

    let interval = Duration::from_secs((settings.poll_interval_seconds.max(1)) as u64);
    let mut attempt: u32 = 0;
    let mut last_status = String::new();
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err("cancelled by the user".to_string());
        }
        if let Some(deadline) = deadline {
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "the job did not finish within {}s (last status: {})",
                    overall_timeout.map(|timeout| timeout.as_secs()).unwrap_or(0),
                    if last_status.trim().is_empty() {
                        "unknown"
                    } else {
                        last_status.trim()
                    }
                ));
            }
        }
        tokio::time::sleep(interval).await;
        attempt += 1;

        let poll_request = apply_headers(client.get(&poll_url), &settings.token, &settings.headers);
        let poll_response = poll_request
            .send()
            .await
            .map_err(|e| format!("poll #{attempt} failed: {e}"))?;
        let poll_status_code = poll_response.status();
        let poll_text = poll_response.text().await.unwrap_or_default();
        if !poll_status_code.is_success() {
            let snippet: String = poll_text.chars().take(300).collect();
            return Err(format!("poll #{attempt} returned HTTP {poll_status_code}: {snippet}"));
        }
        let poll_payload: Value = serde_json::from_str(&poll_text)
            .map_err(|e| format!("poll #{attempt} response is not valid JSON: {e}"))?;

        let job_status = json_path(&poll_payload, &settings.poll_status_path)
            .map(value_to_text)
            .unwrap_or_default();
        last_status = job_status.clone();
        emit_progress(
            app,
            task_id,
            format!("poll #{attempt}: status={}\n", job_status.trim()),
        );

        if status_matches(&job_status, &settings.poll_done_values) {
            let result = extract_text(&poll_payload, &settings.result_path)?;
            return Ok(CloudRunOutcome {
                result,
                duration_ms: started.elapsed().as_millis() as u64,
                job_id: Some(job_id.trim().to_string()),
                summary: format!(
                    "cloud agent \"{}\" finished in {:.1}s (job {})",
                    agent.name,
                    started.elapsed().as_secs_f32(),
                    job_id.trim()
                ),
            });
        }
        if status_matches(&job_status, &settings.poll_failed_values) {
            let detail = json_path(&poll_payload, &settings.result_path)
                .map(value_to_text)
                .unwrap_or_else(|| poll_text.chars().take(400).collect());
            return Err(format!("the job reported status '{}': {detail}", job_status.trim()));
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_paths_read_objects_and_arrays() {
        let value: Value = serde_json::json!({
            "choices": [{ "message": { "content": "hi" } }],
            "result": { "output": [1, 2] }
        });
        assert_eq!(
            json_path(&value, "choices.0.message.content").map(value_to_text),
            Some("hi".to_string())
        );
        assert_eq!(
            json_path(&value, "result.output.1").map(value_to_text),
            Some("2".to_string())
        );
        assert!(json_path(&value, "missing.path").is_none());
    }

    #[test]
    fn body_template_escapes_placeholders() {
        let expanded = expand_body(
            "{\"input\":\"{prompt}\"}",
            "line1\nline2 \"quoted\"",
            "agent",
            Path::new("."),
        );
        let parsed: Value = serde_json::from_str(&expanded).expect("valid json");
        assert_eq!(parsed["input"], "line1\nline2 \"quoted\"");
    }

    #[test]
    fn status_matching_ignores_case_and_space() {
        let values = vec!["completed".to_string(), "done".to_string()];
        assert!(status_matches(" Completed ", &values));
        assert!(!status_matches("running", &values));
    }
}
