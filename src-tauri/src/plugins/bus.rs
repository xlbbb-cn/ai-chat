//! The plugin message bus (PMB).
//!
//! This is the middle hop of `LLM -> tool -> bus -> plugin instance x N`. It
//! owns the three things a fan-out channel needs and nothing else:
//!
//! * **Correlation** — one `request` gets one `reply`. A request id is minted
//!   here and the waiting task holds a `oneshot::Receiver`; the instance reader
//!   task resolves it when the line arrives (`complete`). A caller that gave up
//!   (`expire`) removes the entry, so a late reply is logged instead of waking
//!   nobody.
//! * **Topics** — `a/b/c` with MQTT-style `+` (one level) and `#` (the rest,
//!   terminal). Same semantics as the mission bus (`agents/mbus.rs`), because
//!   two different wildcard grammars in one app would be a trap.
//! * **Audit** — a bounded ring of records (`request` / `reply` / `event` /
//!   `timeout` / `rejected`) the UI can show and `plugin logs` can print. The
//!   bus never persists: a plugin's own state lives in its process.
//!
//! Deliberately synchronous (a `std::sync::Mutex` held only for hashmap /
//! ring edits, never across an `await`): every method is called from a reader
//! task that must not block the executor, and none of them do work that could
//! park.

use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tokio::sync::oneshot;

/// How many requests may be outstanding across every plugin at once.
pub const MAX_PENDING_REQUESTS: usize = 256;
/// Audit ring size (oldest record dropped first).
pub const MAX_BUS_RECORDS: usize = 200;

/// One bus message class, as shown in the audit ring / the UI panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BusKind {
    /// A tool call asked an instance something.
    Request,
    /// The instance answered.
    Reply,
    /// A fire-and-forget publish that reached at least one instance.
    Event,
    /// The caller stopped waiting before a reply arrived.
    Timeout,
    /// The bus refused the message (no instance, dead plugin, cap hit…).
    Rejected,
}

/// One line of the bus audit ring.
#[derive(Debug, Clone, Serialize)]
pub struct BusRecord {
    pub seq: u64,
    pub ts_ms: u64,
    pub kind: BusKind,
    pub plugin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    pub topic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recipients: Vec<String>,
    pub detail: String,
}

/// What an instance sent back for one request.
#[derive(Debug, Clone)]
pub struct Reply {
    pub ok: bool,
    pub result: Value,
    pub error: Option<String>,
}

impl Reply {
    /// The text handed back to the model. A string result passes through
    /// unchanged (the common case: a plugin formats its own answer); anything
    /// else is rendered as pretty JSON so the model can read the structure.
    pub fn text(&self) -> String {
        if !self.ok {
            return format!(
                "Plugin error: {}",
                self.error
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "the instance reported a failure".to_string())
            );
        }
        match &self.result {
            Value::String(text) => text.clone(),
            Value::Null => "OK".to_string(),
            other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
        }
    }
}

/// A request that is waiting for its reply.
pub struct PendingRequest {
    pub id: String,
    pub topic: String,
    pub rx: oneshot::Receiver<Reply>,
}

struct PendingEntry {
    tx: oneshot::Sender<Reply>,
    plugin: String,
    instance: Option<String>,
    topic: String,
    tool: String,
    started: std::time::Instant,
}

struct BusInner {
    pending: HashMap<String, PendingEntry>,
    records: VecDeque<BusRecord>,
}

/// The in-process message bus shared by every plugin instance of the app.
pub struct PluginBus {
    seq: AtomicU64,
    inner: Mutex<BusInner>,
}

impl Default for PluginBus {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginBus {
    pub fn new() -> Self {
        Self {
            seq: AtomicU64::new(1),
            inner: Mutex::new(BusInner {
                pending: HashMap::new(),
                records: VecDeque::with_capacity(MAX_BUS_RECORDS),
            }),
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst)
    }

    /// Mint a request id and register the reply slot.
    ///
    /// Returns `None` when the pending table is full — the caller answers the
    /// model with "bus saturated" instead of queueing unbounded work.
    pub fn begin(&self, plugin: &str, instance: &str, topic: &str, tool: &str) -> Option<PendingRequest> {
        let (tx, rx) = oneshot::channel();
        let id = format!("p{}", self.next_seq());
        let mut inner = self.inner.lock().unwrap();
        if inner.pending.len() >= MAX_PENDING_REQUESTS {
            return None;
        }
        inner.pending.insert(
            id.clone(),
            PendingEntry {
                tx,
                plugin: plugin.to_string(),
                instance: Some(instance.to_string()),
                topic: topic.to_string(),
                tool: tool.to_string(),
                started: std::time::Instant::now(),
            },
        );
        let record = BusRecord {
            seq: self.next_seq(),
            ts_ms: crate::util::now_ms_u64(),
            kind: BusKind::Request,
            plugin: plugin.to_string(),
            instance: Some(instance.to_string()),
            topic: topic.to_string(),
            tool: Some(tool.to_string()),
            ok: None,
            duration_ms: None,
            recipients: vec![],
            detail: format!("request {id}"),
        };
        self.push_record_locked(&mut inner, record);
        Some(PendingRequest {
            id,
            topic: topic.to_string(),
            rx,
        })
    }

    /// Resolve a pending request. Returns the audit record to emit, or `None`
    /// when the id is unknown (a reply that arrived after the caller gave up).
    pub fn complete(&self, id: &str, reply: Reply) -> Option<BusRecord> {
        let entry = {
            let mut inner = self.inner.lock().unwrap();
            inner.pending.remove(id)
        }?;
        let duration_ms = entry.started.elapsed().as_millis() as u64;
        let ok = reply.ok;
        let tx = entry.tx;
        let _ = tx.send(reply);
        let mut inner = self.inner.lock().unwrap();
        let record = BusRecord {
            seq: self.next_seq(),
            ts_ms: crate::util::now_ms_u64(),
            kind: BusKind::Reply,
            plugin: entry.plugin,
            instance: entry.instance,
            topic: entry.topic,
            tool: Some(entry.tool),
            ok: Some(ok),
            duration_ms: Some(duration_ms),
            recipients: vec![],
            detail: format!("reply {id}"),
        };
        self.push_record_locked(&mut inner, record.clone());
        Some(record)
    }

    /// Drop a pending request because the caller timed out. A late reply for
    /// this id is discarded by [`PluginBus::complete`] returning `None`.
    pub fn expire(&self, id: &str) -> Option<BusRecord> {
        let entry = {
            let mut inner = self.inner.lock().unwrap();
            inner.pending.remove(id)
        }?;
        let duration_ms = entry.started.elapsed().as_millis() as u64;
        let mut inner = self.inner.lock().unwrap();
        let record = BusRecord {
            seq: self.next_seq(),
            ts_ms: crate::util::now_ms_u64(),
            kind: BusKind::Timeout,
            plugin: entry.plugin,
            instance: entry.instance,
            topic: entry.topic,
            tool: Some(entry.tool),
            ok: Some(false),
            duration_ms: Some(duration_ms),
            recipients: vec![],
            detail: format!("no reply for {id} within {duration_ms} ms"),
        };
        self.push_record_locked(&mut inner, record.clone());
        Some(record)
    }

    /// Record a fan-out publish.
    pub fn record_event(
        &self,
        plugin: &str,
        topic: &str,
        recipients: &[String],
        detail: String,
    ) -> BusRecord {
        let record = BusRecord {
            seq: self.next_seq(),
            ts_ms: crate::util::now_ms_u64(),
            kind: BusKind::Event,
            plugin: plugin.to_string(),
            instance: None,
            topic: topic.to_string(),
            tool: None,
            ok: Some(!recipients.is_empty()),
            duration_ms: None,
            recipients: recipients.to_vec(),
            detail,
        };
        let mut inner = self.inner.lock().unwrap();
        self.push_record_locked(&mut inner, record.clone());
        record
    }

    /// Record a message the bus refused to route.
    pub fn record_rejected(&self, plugin: &str, topic: &str, detail: String) -> BusRecord {
        let record = BusRecord {
            seq: self.next_seq(),
            ts_ms: crate::util::now_ms_u64(),
            kind: BusKind::Rejected,
            plugin: plugin.to_string(),
            instance: None,
            topic: topic.to_string(),
            tool: None,
            ok: Some(false),
            duration_ms: None,
            recipients: vec![],
            detail,
        };
        let mut inner = self.inner.lock().unwrap();
        self.push_record_locked(&mut inner, record.clone());
        record
    }

    fn push_record_locked(&self, inner: &mut BusInner, record: BusRecord) {
        if inner.records.len() >= MAX_BUS_RECORDS {
            inner.records.pop_front();
        }
        inner.records.push_back(record);
    }

    /// Newest-last slice of the audit ring (`limit` = 0 → everything).
    pub fn records(&self, limit: usize) -> Vec<BusRecord> {
        let inner = self.inner.lock().unwrap();
        let all: Vec<BusRecord> = inner.records.iter().cloned().collect();
        if limit == 0 || limit >= all.len() {
            return all;
        }
        all[all.len() - limit..].to_vec()
    }

    pub fn pending_len(&self) -> usize {
        self.inner.lock().unwrap().pending.len()
    }
}

/// Normalise a topic: lowercase, `/`-separated, `[a-z0-9._-]` per level.
///
/// `allow_wildcards` is on for subscriptions (`+` / `#` levels) and off for a
/// published topic — publishing *to* a wildcard would fan out to an arbitrary
/// set of peers, which is what `publish` already does explicitly.
pub fn normalize_topic(raw: &str, allow_wildcards: bool) -> Result<String, String> {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Err("The topic is empty.".to_string());
    }
    if trimmed.len() > 120 {
        return Err("The topic is longer than 120 characters.".to_string());
    }
    let mut levels: Vec<String> = Vec::new();
    for level in trimmed.split('/') {
        let level = level.trim();
        if level.is_empty() {
            return Err(format!("Topic '{raw}' has an empty level."));
        }
        if level == "+" || level == "#" {
            if !allow_wildcards {
                return Err(format!(
                    "Topic '{raw}' must be concrete; wildcards belong in a subscription."
                ));
            }
            levels.push(level.to_string());
            continue;
        }
        if !level
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        {
            return Err(format!(
                "Topic level '{level}' may only use letters, digits, '.', '_' and '-'."
            ));
        }
        levels.push(level.to_ascii_lowercase());
    }
    Ok(levels.join("/"))
}

/// MQTT-style filter match: `+` matches exactly one level, a terminal `#`
/// matches the rest (including nothing).
pub fn topic_matches(filter: &str, topic: &str) -> bool {
    if filter == "#" {
        return true;
    }
    let filter_levels: Vec<&str> = filter.split('/').collect();
    let topic_levels: Vec<&str> = topic.split('/').collect();
    let mut index = 0;
    for (position, level) in filter_levels.iter().enumerate() {
        if *level == "#" {
            // `#` is terminal; a non-terminal `#` still matches greedily.
            return position + 1 == filter_levels.len();
        }
        let Some(candidate) = topic_levels.get(index) else {
            return false;
        };
        if *level != "+" && !level.eq_ignore_ascii_case(candidate) {
            return false;
        }
        index += 1;
    }
    index == topic_levels.len()
}
