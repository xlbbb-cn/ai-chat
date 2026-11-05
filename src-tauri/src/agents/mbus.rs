//! Mission bus (`mbus`) — a small, in-process MQTT-like pub/sub channel that
//! lets the planner supervise a mission and lets its sub-agents coordinate.
//!
//! One bus exists per orchestration batch (chat turn). It is created after the
//! planner produced a task plan, lives on the `Arc` passed to every task, and
//! is discarded when the batch ends. Messages are queued per peer and drained
//! by the sub-agent loop at its next request boundary — exactly like the main
//! agent's mid-turn injection. Nothing here talks to an external broker.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use super::{
    get_active_mission_tasks, log_agent_event, normalize_agent_type, preview_text, SubAgent, Task,
    TaskResult, AGENT_TYPE_LLM,
};
use crate::AppState;

/// Peer id of the planner. Plan task ids are sanitized so they can never
/// collide with this reserved id (see `parse_task_plan`).
pub const PLANNER_ID: &str = "planner";

/// Prefix attached to every message injected into a sub-agent session. It must
/// read as peer coordination, not as a new user request, because some models
/// otherwise start answering the message instead of continuing the mission.
pub const MBUS_INJECTION_PREFIX: &str = "[MISSION BUS MESSAGE — from a peer agent on this mission's message bus. This is coordination information, NOT a new user request; the mission instructions remain authoritative. Continue your own task.]";

/// How many queued messages one reasoning round may absorb. The rest stay in
/// the inbox for the next round instead of blowing up the context in one go.
pub const MAX_DELIVERIES_PER_ROUND: usize = 8;

const MAX_BODY_CHARS: usize = 2_000;
const MAX_MESSAGES_PER_BUS: usize = 64;
const MAX_MESSAGES_PER_PEER: usize = 8;
const PLANNER_MAX_MESSAGES: usize = 16;
const MAX_INBOX: usize = 16;
const MAX_SUBSCRIPTIONS: usize = 16;
const MAX_TOPIC_LEN: usize = 120;
const MAX_HISTORY: usize = 200;
const MAX_ACTIVITY: usize = 200;
const MAX_TOOL_RESULT_BYTES: usize = 16_000;
const MAX_SUMMARY_CHARS: usize = 600;
const MAX_STATUS_ACTIVE_TASKS: usize = 5;

// ─── Public types ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
pub struct BusPeer {
    pub id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub description: String,
    pub cwd: Option<String>,
    pub dependencies: Vec<String>,
    /// `llm` agents can receive a message at any time. `cli` / `cloud` agents
    /// only before they start (their prompt absorbs the queued text once).
    pub can_receive: bool,
    pub is_planner: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PeerStatus {
    pub id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub description: String,
    pub cwd: Option<String>,
    pub dependencies: Vec<String>,
    pub state: String,
    pub can_receive: bool,
    pub iterations: u32,
    pub tool_calls: u32,
    pub tokens: u32,
    pub last_tool: Option<String>,
    pub last_activity_ms: i64,
    pub queued_messages: usize,
    pub summary: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct BusMessage {
    pub id: String,
    pub seq: u64,
    pub from_id: String,
    pub from_name: String,
    pub target_kind: String,
    pub target_id: String,
    pub topic: String,
    pub body: String,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug)]
pub struct PublishedMessage {
    pub id: String,
    pub seq: u64,
    pub from_id: String,
    pub from_name: String,
    pub target_kind: String,
    pub target_id: String,
    pub topic: String,
    pub body: String,
    pub recipients: Vec<BusPeer>,
    pub skipped: Vec<(String, String)>,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ActivityEvent {
    pub seq: u64,
    pub at_ms: i64,
    pub kind: String,
    pub task_id: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug)]
pub enum BusTarget {
    Direct(String),
    Topic(String),
}

struct PeerState {
    peer: BusPeer,
    state: String,
    iterations: u32,
    tool_calls: u32,
    tokens: u32,
    last_tool: Option<String>,
    last_activity_ms: i64,
    summary: String,
    inbox: VecDeque<BusMessage>,
    subscriptions: HashSet<String>,
    sent_count: usize,
    delivered_count: usize,
    started_at_ms: Option<i64>,
    finished_at_ms: Option<i64>,
}

impl PeerState {
    fn new(peer: BusPeer, state: &str, subscriptions: &[&str]) -> Self {
        Self {
            peer,
            state: state.to_string(),
            iterations: 0,
            tool_calls: 0,
            tokens: 0,
            last_tool: None,
            last_activity_ms: now_ms(),
            summary: String::new(),
            inbox: VecDeque::new(),
            subscriptions: subscriptions.iter().map(|s| s.to_string()).collect(),
            sent_count: 0,
            delivered_count: 0,
            started_at_ms: None,
            finished_at_ms: None,
        }
    }
}

struct BusInner {
    /// Insertion order (planner first, then the plan's task order) so status
    /// output is stable and readable instead of hash-map order.
    order: Vec<String>,
    peers: HashMap<String, PeerState>,
    history: VecDeque<BusMessage>,
    activity: VecDeque<ActivityEvent>,
    total_messages: usize,
    seq: u64,
    activity_seq: u64,
    closed: bool,
}

/// One mission's message bus. Cheap to clone through `Arc`.
pub struct MissionBus {
    bus_id: String,
    session_id: String,
    inner: Mutex<BusInner>,
    notify: tokio::sync::Notify,
}

impl MissionBus {
    pub fn new(session_id: &str, tasks: &[Task], agents: &[SubAgent]) -> Self {
        let mut order: Vec<String> = Vec::with_capacity(tasks.len() + 1);
        let mut peers: HashMap<String, PeerState> = HashMap::new();

        order.push(PLANNER_ID.to_string());
        peers.insert(
            PLANNER_ID.to_string(),
            PeerState::new(
                BusPeer {
                    id: PLANNER_ID.to_string(),
                    agent_id: PLANNER_ID.to_string(),
                    agent_name: "Planner".to_string(),
                    description: "Mission planner and supervisor".to_string(),
                    cwd: None,
                    dependencies: Vec::new(),
                    can_receive: true,
                    is_planner: true,
                },
                "running",
                &["broadcast", "planner"],
            ),
        );

        let mut seen: HashSet<String> = HashSet::new();
        for task in tasks {
            if task.id.trim().is_empty() || !seen.insert(task.id.clone()) {
                continue;
            }
            let agent = agents.iter().find(|a| a.id == task.agent_id);
            let kind = agent
                .map(|a| normalize_agent_type(&a.agent_type))
                .unwrap_or_default();
            let can_receive = kind == AGENT_TYPE_LLM;
            let agent_name = agent
                .map(|a| a.name.trim().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| task.agent_id.clone());
            let description = agent
                .map(|a| a.description.trim().to_string())
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| task.description.clone());

            order.push(task.id.clone());
            peers.insert(
                task.id.clone(),
                PeerState::new(
                    BusPeer {
                        id: task.id.clone(),
                        agent_id: task.agent_id.clone(),
                        agent_name,
                        description,
                        cwd: task.cwd.clone(),
                        dependencies: task.dependencies.clone(),
                        can_receive,
                        is_planner: false,
                    },
                    "pending",
                    // External agents absorb queued text once, when their
                    // prompt is built; the broadcast subscription is harmless.
                    &["broadcast"],
                ),
            );
        }

        Self {
            bus_id: format!("mbus-{}", Uuid::new_v4().simple()),
            session_id: session_id.to_string(),
            inner: Mutex::new(BusInner {
                order,
                peers,
                history: VecDeque::new(),
                activity: VecDeque::new(),
                total_messages: 0,
                seq: 0,
                activity_seq: 0,
                closed: false,
            }),
            notify: tokio::sync::Notify::new(),
        }
    }

    pub fn bus_id(&self) -> &str {
        &self.bus_id
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// True when at least one peer can run the in-app LLM loop. A batch made
    /// only of cli/cloud agents has no one to message; the orchestration skips
    /// the bus and the supervisor entirely.
    pub fn has_llm_peer(&self) -> bool {
        let inner = self.lock();
        inner
            .peers
            .values()
            .any(|peer| peer.peer.can_receive && !peer.peer.is_planner)
    }

    pub fn agents(&self) -> Vec<BusPeer> {
        let inner = self.lock();
        inner
            .order
            .iter()
            .filter_map(|id| inner.peers.get(id))
            .filter(|peer| !peer.peer.is_planner)
            .map(|peer| peer.peer.clone())
            .collect()
    }

    pub fn planner(&self) -> Option<BusPeer> {
        let inner = self.lock();
        inner.peers.get(PLANNER_ID).map(|peer| peer.peer.clone())
    }

    pub fn peer(&self, id: &str) -> Option<BusPeer> {
        let inner = self.lock();
        inner.peers.get(id).map(|peer| peer.peer.clone())
    }

    /// Resolve a user/model supplied target: a task id, an agent id (when it
    /// uniquely identifies one task), `planner`, or the broadcast aliases.
    pub fn resolve_direct(&self, target: &str) -> Result<BusPeer, String> {
        let trimmed = target.trim();
        if trimmed.is_empty() {
            return Err("target cannot be empty".to_string());
        }
        if matches!(trimmed, "all" | "*" | "broadcast") {
            return Err("use the topic 'broadcast' for a broadcast message".to_string());
        }
        let inner = self.lock();
        if let Some(peer) = inner.peers.get(trimmed) {
            return Ok(peer.peer.clone());
        }
        let candidates: Vec<&PeerState> = inner
            .peers
            .values()
            .filter(|peer| !peer.peer.is_planner && peer.peer.agent_id == trimmed)
            .collect();
        match candidates.as_slice() {
            [one] => Ok(one.peer.clone()),
            [] => {
                let ids = inner
                    .order
                    .iter()
                    .filter(|id| *id != PLANNER_ID)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(format!(
                    "unknown mbus target '{trimmed}'. Use one of: {ids}, planner"
                ))
            }
            _ => Err(format!(
                "agent id '{trimmed}' is used by more than one task; address the task id instead: {}",
                candidates
                    .iter()
                    .map(|peer| peer.peer.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    pub fn publish(
        &self,
        from_id: &str,
        target: BusTarget,
        body: &str,
    ) -> Result<PublishedMessage, String> {
        let text = body.trim().to_string();
        if text.is_empty() {
            return Err("message body cannot be empty".to_string());
        }
        if text.chars().count() > MAX_BODY_CHARS {
            return Err(format!(
                "message body is too long ({} chars, max {MAX_BODY_CHARS})",
                text.chars().count()
            ));
        }

        let mut inner = self.lock();
        if inner.closed {
            return Err("the mission bus is already closed".to_string());
        }
        let sender = inner
            .peers
            .get(from_id)
            .map(|peer| peer.peer.clone())
            .ok_or_else(|| format!("unknown mbus sender '{from_id}'"))?;
        let limit = if sender.is_planner {
            PLANNER_MAX_MESSAGES
        } else {
            MAX_MESSAGES_PER_PEER
        };
        let sent = inner
            .peers
            .get(from_id)
            .map(|peer| peer.sent_count)
            .unwrap_or(0);
        if sent >= limit {
            return Err(format!(
                "message limit reached for '{from_id}' ({limit} messages per mission)"
            ));
        }
        if inner.total_messages >= MAX_MESSAGES_PER_BUS {
            return Err(format!(
                "mission message limit reached ({MAX_MESSAGES_PER_BUS} messages)"
            ));
        }

        let mut skipped_out: Vec<(String, String)> = Vec::new();
        let (target_kind, target_id, topic, recipients): (String, String, String, Vec<BusPeer>) =
            match target {
                BusTarget::Direct(raw_target) => {
                    let peer = resolve_direct_locked(&inner, &raw_target)?;
                    if peer.id == sender.id {
                        return Err("a peer cannot send an mbus message to itself".to_string());
                    }
                    let state = inner
                        .peers
                        .get(&peer.id)
                        .map(|entry| entry.state.clone())
                        .unwrap_or_else(|| "pending".to_string());
                    if !can_accept_now(&peer, &state) {
                        return Err(if is_terminal(&state) {
                            format!(
                                "'{}' has already finished; it cannot receive messages",
                                peer.id
                            )
                        } else if peer.can_receive {
                            format!("'{}' cannot receive messages in state '{}'", peer.id, state)
                        } else {
                            format!(
                            "'{}' is a status-only peer while running; external agents only accept queued messages before they start",
                            peer.id
                        )
                        });
                    }
                    let inbox_full = inner
                        .peers
                        .get(&peer.id)
                        .map(|entry| entry.inbox.len() >= MAX_INBOX)
                        .unwrap_or(false);
                    if inbox_full {
                        return Err(format!(
                            "the inbox of '{}' is full ({MAX_INBOX} queued messages)",
                            peer.id
                        ));
                    }
                    (
                        String::from("direct"),
                        peer.id.clone(),
                        String::new(),
                        vec![peer],
                    )
                }
                BusTarget::Topic(raw_topic) => {
                    let topic = normalize_topic(&raw_topic, false)?;
                    let mut selected = Vec::new();
                    for id in &inner.order {
                        if id == &sender.id {
                            continue;
                        }
                        let Some(peer) = inner.peers.get(id) else {
                            continue;
                        };
                        if !peer
                            .subscriptions
                            .iter()
                            .any(|filter| topic_matches(filter, &topic))
                        {
                            continue;
                        }
                        if !can_accept_now(&peer.peer, &peer.state) {
                            continue;
                        }
                        selected.push((id.clone(), peer.peer.clone()));
                    }

                    let mut recipients = Vec::new();
                    let mut skipped = Vec::new();
                    for (id, peer) in selected {
                        let full = inner
                            .peers
                            .get(&id)
                            .map(|entry| entry.inbox.len() >= MAX_INBOX)
                            .unwrap_or(true);
                        if full {
                            skipped.push((peer.id.clone(), "inbox full".to_string()));
                        } else {
                            recipients.push(peer);
                        }
                    }
                    if recipients.is_empty() {
                        let suffix = if skipped.is_empty() {
                            "no peer is currently subscribed to that topic and able to receive messages".to_string()
                        } else {
                            format!(
                                "no message was delivered: {}",
                                skipped
                                    .iter()
                                    .map(|(id, reason)| format!("{id} ({reason})"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        return Err(format!("topic '{topic}': {suffix}"));
                    }
                    skipped_out = skipped;
                    (String::from("topic"), topic.clone(), topic, recipients)
                }
            };

        let seq = next_seq(&mut inner);
        let created_at_ms = now_ms();
        let message = BusMessage {
            id: Uuid::new_v4().to_string(),
            seq,
            from_id: sender.id.clone(),
            from_name: sender.agent_name.clone(),
            target_kind: target_kind.clone(),
            target_id: target_id.clone(),
            topic: topic.clone(),
            body: text.clone(),
            created_at_ms,
        };

        for recipient in &recipients {
            if let Some(peer) = inner.peers.get_mut(&recipient.id) {
                peer.inbox.push_back(message.clone());
            }
        }
        if let Some(peer) = inner.peers.get_mut(&sender.id) {
            peer.sent_count = peer.sent_count.saturating_add(1);
        }
        inner.total_messages = inner.total_messages.saturating_add(1);
        push_history(&mut inner, message.clone());
        let target_label = if target_kind == "topic" {
            format!("#{topic}")
        } else {
            target_id.clone()
        };
        push_activity(
            &mut inner,
            seq,
            message_kind(&sender, &target_kind, &target_id, &topic),
            Some(sender.id.clone()),
            format!(
                "{} -> {}: {}",
                sender.agent_name,
                target_label,
                preview_text(&text, 160)
            ),
        );

        // Keep the "skipped" list for the tool result even though the message
        // is delivered to at least one peer.
        let skipped = skipped_out;
        drop(inner);
        self.notify.notify_waiters();

        Ok(PublishedMessage {
            id: message.id,
            seq,
            from_id: message.from_id,
            from_name: message.from_name,
            target_kind,
            target_id,
            topic,
            body: text,
            recipients,
            skipped,
            created_at_ms,
        })
    }

    pub fn take_inbox(&self, peer_id: &str, limit: usize) -> Vec<BusMessage> {
        let mut inner = self.lock();
        let Some(peer) = inner.peers.get_mut(peer_id) else {
            return Vec::new();
        };
        let take = limit.min(peer.inbox.len());
        let messages: Vec<BusMessage> = peer.inbox.drain(..take).collect();
        peer.delivered_count = peer.delivered_count.saturating_add(messages.len());
        peer.last_activity_ms = now_ms();
        messages
    }

    pub fn inbox_len(&self, peer_id: &str) -> usize {
        let inner = self.lock();
        inner
            .peers
            .get(peer_id)
            .map(|peer| peer.inbox.len())
            .unwrap_or(0)
    }

    pub fn subscribe(&self, peer_id: &str, topics: &[String]) -> Result<Vec<String>, String> {
        let mut inner = self.lock();
        let Some(peer) = inner.peers.get_mut(peer_id) else {
            return Err(format!("unknown mbus peer '{peer_id}'"));
        };
        for topic in topics {
            let normalized = normalize_topic(topic, true)?;
            if peer.subscriptions.len() >= MAX_SUBSCRIPTIONS
                && !peer.subscriptions.contains(&normalized)
            {
                return Err(format!(
                    "subscription limit reached ({MAX_SUBSCRIPTIONS} topics)"
                ));
            }
            peer.subscriptions.insert(normalized);
        }
        let mut current: Vec<String> = peer.subscriptions.iter().cloned().collect();
        current.sort();
        Ok(current)
    }

    pub fn unsubscribe(&self, peer_id: &str, topics: &[String]) -> Result<Vec<String>, String> {
        let mut inner = self.lock();
        let Some(peer) = inner.peers.get_mut(peer_id) else {
            return Err(format!("unknown mbus peer '{peer_id}'"));
        };
        for topic in topics {
            let normalized = normalize_topic(topic, true)?;
            if normalized == "broadcast" {
                return Err("'broadcast' is required and cannot be unsubscribed".to_string());
            }
            peer.subscriptions.remove(&normalized);
        }
        let mut current: Vec<String> = peer.subscriptions.iter().cloned().collect();
        current.sort();
        Ok(current)
    }

    /// Recent traffic as seen by `viewer_id`. The planner sees everything; an
    /// agent sees its own messages, messages addressed to it and traffic on
    /// topics it subscribed to (broadcast is always included).
    pub fn history(&self, viewer_id: &str, topic: Option<&str>, limit: usize) -> Vec<BusMessage> {
        let inner = self.lock();
        let limit = limit.clamp(1, 50);
        let viewer = inner.peers.get(viewer_id);
        let is_planner = viewer.map(|peer| peer.peer.is_planner).unwrap_or(false);
        let normalized_topic = topic
            .map(|value| normalize_topic(value, true))
            .transpose()
            .unwrap_or(None);

        let mut out = Vec::new();
        for message in inner.history.iter().rev() {
            if out.len() >= limit {
                break;
            }
            if let Some(filter) = &normalized_topic {
                let topic_hit = if message.target_kind == "topic" {
                    topic_matches(filter, &message.topic)
                } else {
                    false
                };
                if !topic_hit {
                    continue;
                }
            }
            if !is_planner && !viewer_can_see(message, viewer) {
                continue;
            }
            out.push(message.clone());
        }
        out
    }

    pub fn status(&self) -> Vec<PeerStatus> {
        let inner = self.lock();
        let now = now_ms();
        let mut out = Vec::new();
        for id in &inner.order {
            let Some(peer) = inner.peers.get(id) else {
                continue;
            };
            if peer.peer.is_planner {
                continue;
            }
            out.push(PeerStatus {
                id: peer.peer.id.clone(),
                agent_id: peer.peer.agent_id.clone(),
                agent_name: peer.peer.agent_name.clone(),
                description: peer.peer.description.clone(),
                cwd: peer.peer.cwd.clone(),
                dependencies: peer.peer.dependencies.clone(),
                state: peer.state.clone(),
                can_receive: peer.peer.can_receive,
                iterations: peer.iterations,
                tool_calls: peer.tool_calls,
                tokens: peer.tokens,
                last_tool: peer.last_tool.clone(),
                last_activity_ms: now.saturating_sub(peer.last_activity_ms).max(0),
                queued_messages: peer.inbox.len(),
                summary: peer.summary.clone(),
            });
        }
        out
    }

    /// Idle time in milliseconds (not an absolute timestamp) so prompts stay
    /// human-readable without a clock calculation by the model.
    pub fn activity_since(&self, seq: u64) -> Vec<ActivityEvent> {
        let inner = self.lock();
        inner
            .activity
            .iter()
            .filter(|event| event.seq > seq)
            .cloned()
            .collect()
    }

    pub fn activity_seq(&self) -> u64 {
        let inner = self.lock();
        inner.activity_seq
    }

    pub fn mark_running(&self, task_id: &str) {
        let mut inner = self.lock();
        let agent_name = {
            let Some(peer) = inner.peers.get_mut(task_id) else {
                return;
            };
            if peer.state != "pending" {
                return;
            }
            peer.state = "running".to_string();
            peer.started_at_ms = Some(now_ms());
            peer.last_activity_ms = now_ms();
            peer.peer.agent_name.clone()
        };
        let seq = next_seq(&mut inner);
        push_activity(
            &mut inner,
            seq,
            "task_started",
            Some(task_id.to_string()),
            format!("{} ({}) started", agent_name, task_id),
        );
        drop(inner);
        self.notify.notify_waiters();
    }

    pub fn mark_iteration(&self, task_id: &str, iterations: u32, tokens: u32) {
        let mut inner = self.lock();
        if let Some(peer) = inner.peers.get_mut(task_id) {
            peer.iterations = iterations;
            peer.tokens = tokens;
            peer.last_activity_ms = now_ms();
        }
    }

    pub fn mark_tool(&self, task_id: &str, tool: &str) {
        let mut inner = self.lock();
        if let Some(peer) = inner.peers.get_mut(task_id) {
            peer.tool_calls = peer.tool_calls.saturating_add(1);
            peer.last_tool = Some(tool.to_string());
            peer.last_activity_ms = now_ms();
        }
    }

    pub fn finish_task(&self, result: &TaskResult) {
        let mut inner = self.lock();
        let (state, summary) = {
            let Some(peer) = inner.peers.get_mut(&result.task_id) else {
                return;
            };
            if is_terminal(&peer.state) {
                return;
            }
            let state = match result.status.as_str() {
                "success" => "completed",
                "error" => "error",
                _ => "skipped",
            };
            peer.state = state.to_string();
            peer.summary = preview_text(result.content.trim(), MAX_SUMMARY_CHARS);
            peer.last_activity_ms = now_ms();
            peer.finished_at_ms = Some(now_ms());
            // Messages that never made it into a reasoning round are dropped
            // with the task; keeping them would only leak memory until the bus
            // dies.
            peer.inbox.clear();
            (state.to_string(), peer.summary.clone())
        };
        let seq = next_seq(&mut inner);
        let kind = if state == "completed" {
            "task_finished"
        } else if state == "error" {
            "task_failed"
        } else {
            "task_skipped"
        };
        push_activity(
            &mut inner,
            seq,
            kind,
            Some(result.task_id.clone()),
            format!(
                "{} ({}) {}{}",
                result.agent_name,
                result.task_id,
                state,
                if summary.is_empty() {
                    String::new()
                } else {
                    format!(": {}", preview_text(&summary, 160))
                }
            ),
        );
        drop(inner);
        self.notify.notify_waiters();
    }

    pub fn mark_skipped(&self, result: &TaskResult) {
        self.finish_task(result);
    }

    /// Wait until the next lifecycle/message notification. The supervisor also
    /// polls a short timeout so Stop and task completion stay responsive even
    /// when no bus event arrives.
    pub async fn notified(&self) {
        self.notify.notified().await;
    }

    pub fn close(&self) {
        let mut inner = self.lock();
        inner.closed = true;
        drop(inner);
        self.notify.notify_waiters();
    }

    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    pub fn total_messages(&self) -> usize {
        self.lock().total_messages
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BusInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn next_seq(inner: &mut BusInner) -> u64 {
    inner.seq = inner.seq.saturating_add(1);
    inner.activity_seq = inner.seq;
    inner.seq
}

fn push_history(inner: &mut BusInner, message: BusMessage) {
    inner.history.push_back(message);
    while inner.history.len() > MAX_HISTORY {
        inner.history.pop_front();
    }
}

fn push_activity(
    inner: &mut BusInner,
    seq: u64,
    kind: &str,
    task_id: Option<String>,
    text: String,
) {
    inner.activity.push_back(ActivityEvent {
        seq,
        at_ms: now_ms(),
        kind: kind.to_string(),
        task_id,
        text,
    });
    while inner.activity.len() > MAX_ACTIVITY {
        inner.activity.pop_front();
    }
}

fn message_kind(sender: &BusPeer, target_kind: &str, target_id: &str, topic: &str) -> &'static str {
    if sender.is_planner {
        return "planner_message";
    }
    if target_kind == "direct" {
        if !sender.is_planner && target_id == PLANNER_ID {
            "message_to_planner"
        } else {
            "message_direct"
        }
    } else if !sender.is_planner && topic == "planner" {
        "message_to_planner"
    } else if topic == "broadcast" {
        "message_broadcast"
    } else {
        "message_topic"
    }
}

fn is_terminal(state: &str) -> bool {
    matches!(state, "completed" | "error" | "skipped")
}

/// Whether `peer` can accept a message in `state`.
///
/// - planner: always.
/// - `llm` peers: pending or running (the loop drains at the next boundary).
/// - `cli` / `cloud` peers: pending only, because `build_external_prompt`
///   absorbs the inbox once when the process starts.
fn can_accept_now(peer: &BusPeer, state: &str) -> bool {
    if peer.is_planner {
        return true;
    }
    if is_terminal(state) {
        return false;
    }
    if peer.can_receive {
        matches!(state, "pending" | "running")
    } else {
        state == "pending"
    }
}

fn resolve_direct_locked(inner: &BusInner, target: &str) -> Result<BusPeer, String> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err("target cannot be empty".to_string());
    }
    if matches!(trimmed, "all" | "*" | "broadcast") {
        return Err("use the topic 'broadcast' for a broadcast message".to_string());
    }
    if let Some(peer) = inner.peers.get(trimmed) {
        return Ok(peer.peer.clone());
    }
    let candidates: Vec<&PeerState> = inner
        .peers
        .values()
        .filter(|peer| !peer.peer.is_planner && peer.peer.agent_id == trimmed)
        .collect();
    match candidates.as_slice() {
        [one] => Ok(one.peer.clone()),
        [] => {
            let ids = inner
                .order
                .iter()
                .filter(|id| *id != PLANNER_ID)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "unknown mbus target '{trimmed}'. Use one of: {ids}, planner"
            ))
        }
        _ => Err(format!(
            "agent id '{trimmed}' is used by more than one task; address the task id instead: {}",
            candidates
                .iter()
                .map(|peer| peer.peer.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn viewer_can_see(message: &BusMessage, viewer: Option<&PeerState>) -> bool {
    let Some(viewer) = viewer else {
        return false;
    };
    if message.from_id == viewer.peer.id {
        return true;
    }
    if message.target_kind == "direct" {
        return message.target_id == viewer.peer.id;
    }
    if message.topic == "broadcast" {
        return true;
    }
    viewer
        .subscriptions
        .iter()
        .any(|filter| topic_matches(filter, &message.topic))
}

/// Validate a topic / subscription filter. Publication topics may not contain
/// MQTT wildcards; subscriptions may.
pub fn normalize_topic(raw: &str, allow_wildcards: bool) -> Result<String, String> {
    let topic = raw.trim();
    if topic.is_empty() {
        return Err("topic cannot be empty".to_string());
    }
    if topic.len() > MAX_TOPIC_LEN {
        return Err(format!(
            "topic is too long ({} chars, max {MAX_TOPIC_LEN})",
            topic.len()
        ));
    }
    if matches!(topic, "all" | "*") {
        return Ok("broadcast".to_string());
    }
    if topic.starts_with('/')
        || topic.ends_with('/')
        || topic.split('/').any(|part| part.is_empty())
    {
        return Err("topic segments cannot be empty".to_string());
    }
    for (index, part) in topic.split('/').enumerate() {
        if part == "#" {
            if !allow_wildcards {
                return Err("'#' is only valid in a subscription filter".to_string());
            }
            if topic.split('/').count() != index + 1 {
                return Err("'#' must be the last subscription segment".to_string());
            }
        } else if part == "+" {
            if !allow_wildcards {
                return Err("'+' is only valid in a subscription filter".to_string());
            }
        } else if part.contains('#') || part.contains('+') {
            return Err("wildcards must occupy a whole topic segment".to_string());
        }
    }
    Ok(topic.to_string())
}

/// MQTT-style topic matching (`+` = one segment, `#` = the remaining segments).
pub fn topic_matches(filter: &str, topic: &str) -> bool {
    let filter_parts: Vec<&str> = filter.split('/').collect();
    let topic_parts: Vec<&str> = topic.split('/').collect();
    let mut fi = 0;
    let mut ti = 0;
    while fi < filter_parts.len() {
        match filter_parts[fi] {
            "#" => return true,
            "+" => {
                if ti >= topic_parts.len() {
                    return false;
                }
            }
            literal => {
                if ti >= topic_parts.len() || literal != topic_parts[ti] {
                    return false;
                }
            }
        }
        fi += 1;
        ti += 1;
    }
    ti == topic_parts.len()
}

// ─── Tools ────────────────────────────────────────────────────────────────────

/// OpenAI tool definition for the mission bus. Sub-agents receive every action
/// except `finish`; the planner supervisor gets `finish` as well.
pub fn tool_definition(for_planner: bool) -> Value {
    let mut actions = vec!["send", "status", "history", "subscribe", "unsubscribe"];
    if for_planner {
        actions.push("finish");
    }
    let description = if for_planner {
        "Mission message bus (mbus). Inspect peer agents' live status and send short coordination notes while the mission runs. Messages are queued and delivered to an agent at its NEXT reasoning round — never instantly. Use `status` before nudging, do not repeat guidance, and call `finish` when there is nothing left to supervise."
    } else {
        "Mission message bus (mbus). Coordinate with peer agents on this mission: inspect their status, send a short direct note to one task, publish to a topic (broadcast reaches everyone), or read recent traffic. Messages are delivered at the recipient's NEXT reasoning round, never instantly; do not wait for an immediate reply and do not send acknowledgements."
    };
    json!({
        "type": "function",
        "function": {
            "name": "mbus",
            "description": description,
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": actions,
                        "description": "send = queue a message; status = live peer state; history = recent traffic; subscribe/unsubscribe = topic filters; finish = stop supervising (planner only)."
                    },
                    "to": {
                        "type": "string",
                        "description": "Direct recipient: a task id such as \"t2\", an agent id when it is unique, or \"planner\"."
                    },
                    "topic": {
                        "type": "string",
                        "description": "Topic for send/history/subscribe. \"broadcast\" reaches every agent; subscriptions support MQTT wildcards + and #. Publication topics cannot contain wildcards."
                    },
                    "message": {
                        "type": "string",
                        "description": "Message body (max 2000 characters). Use a short, actionable note."
                    },
                    "topics": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Topics to subscribe/unsubscribe (subscriptions support + and #; broadcast is always subscribed)."
                    },
                    "task_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional status filter: task ids or agent ids. Omit for every peer."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "History limit (1–50, default 20)."
                    },
                    "note": {
                        "type": "string",
                        "description": "Optional note for the finish action."
                    }
                },
                "required": ["action"]
            }
        }
    })
}

/// Handle one `mbus` tool call. Returns the text placed in the model's `tool`
/// message. `finish` is handled by the supervisor loop; here it only produces
/// the acknowledgement.
pub fn handle_tool(
    app: &AppHandle,
    bus: &MissionBus,
    viewer_id: &str,
    args: &Value,
    for_planner: bool,
) -> String {
    let action = args["action"].as_str().unwrap_or("").trim();
    match action {
        "send" => {
            let to = args["to"].as_str().unwrap_or("").trim();
            let topic = args["topic"].as_str().unwrap_or("").trim();
            if !to.is_empty() && !topic.is_empty() {
                return "Error: provide either 'to' (direct) or 'topic' (topic), not both."
                    .to_string();
            }
            let body = args["message"].as_str().unwrap_or("");
            let target = if !to.is_empty() {
                if matches!(to, "all" | "*" | "broadcast") {
                    BusTarget::Topic("broadcast".to_string())
                } else {
                    BusTarget::Direct(to.to_string())
                }
            } else {
                BusTarget::Topic(if topic.is_empty() {
                    "broadcast".to_string()
                } else {
                    topic.to_string()
                })
            };
            match publish_and_record(app, bus, viewer_id, target, body) {
                Ok(message) => cap_tool_result(
                    serde_json::to_string_pretty(&json!({
                        "ok": true,
                        "message_id": message.id,
                        "target_kind": message.target_kind,
                        "target_id": message.target_id,
                        "recipients": message.recipients.iter().map(|peer| json!({
                            "id": peer.id,
                            "name": peer.agent_name,
                        })).collect::<Vec<_>>(),
                        "skipped": message.skipped.iter().map(|(id, reason)| json!({
                            "id": id,
                            "reason": reason,
                        })).collect::<Vec<_>>(),
                        "note": "Queued; each recipient receives it at its next reasoning round."
                    }))
                    .unwrap_or_else(|_| "ok".to_string()),
                ),
                Err(err) => format!("Error: {err}"),
            }
        }
        "status" => {
            let filter: Vec<String> = args["task_ids"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str())
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let snapshot = status_snapshot(
                app,
                bus,
                viewer_id,
                if filter.is_empty() {
                    None
                } else {
                    Some(&filter)
                },
            );
            cap_tool_result(
                serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string()),
            )
        }
        "history" => {
            let topic = args["topic"].as_str();
            let limit = args["limit"].as_u64().unwrap_or(20) as usize;
            let messages = bus.history(viewer_id, topic, limit);
            cap_tool_result(
                serde_json::to_string_pretty(&json!({ "messages": messages }))
                    .unwrap_or_else(|_| "{}".to_string()),
            )
        }
        "subscribe" | "unsubscribe" => {
            let topics: Vec<String> = args["topics"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str())
                        .map(|value| value.to_string())
                        .collect()
                })
                .unwrap_or_default();
            if topics.is_empty() {
                return "Error: 'topics' must contain at least one topic.".to_string();
            }
            let result = if action == "subscribe" {
                bus.subscribe(viewer_id, &topics)
            } else {
                bus.unsubscribe(viewer_id, &topics)
            };
            match result {
                Ok(subscriptions) => serde_json::to_string_pretty(&json!({
                    "ok": true,
                    "subscriptions": subscriptions,
                }))
                .unwrap_or_else(|_| "{}".to_string()),
                Err(err) => format!("Error: {err}"),
            }
        }
        "finish" if for_planner => {
            let note = args["note"].as_str().unwrap_or("").trim();
            serde_json::to_string_pretty(&json!({
                "ok": true,
                "note": if note.is_empty() { "supervision finished" } else { note },
            }))
            .unwrap_or_else(|_| "{\"ok\":true}".to_string())
        }
        "" => "Error: 'action' is required.".to_string(),
        other => format!("Error: unsupported mbus action '{other}'."),
    }
}

fn cap_tool_result(text: String) -> String {
    if text.len() <= MAX_TOOL_RESULT_BYTES {
        return text;
    }
    format!(
        "{}\n\n[truncated: mbus result exceeded {MAX_TOOL_RESULT_BYTES} bytes]",
        preview_text(&text, MAX_TOOL_RESULT_BYTES)
    )
}

/// Publish, persist the message so the Mission panel can replay it, and emit
/// the live event. This is the only publish path used by tools and supervisor.
pub fn publish_and_record(
    app: &AppHandle,
    bus: &MissionBus,
    from_id: &str,
    target: BusTarget,
    body: &str,
) -> Result<PublishedMessage, String> {
    let message = bus.publish(from_id, target, body)?;

    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(db) = state.db.lock() {
            let _ = save_bus_message(&db, bus, &message);
        }
    }

    let recipients: Vec<Value> = message
        .recipients
        .iter()
        .map(|peer| json!({ "id": peer.id, "name": peer.agent_name }))
        .collect();
    let payload = json!({
        "bus_id": bus.bus_id(),
        "session_id": bus.session_id(),
        "from_id": message.from_id,
        "from_name": message.from_name,
        "target_kind": message.target_kind,
        "target_id": message.target_id,
        "topic": message.topic,
        "body": message.body,
        "recipients": recipients,
        "created_at_ms": message.created_at_ms,
    });
    let _ = app.emit("agent-mbus-message", payload);
    log_agent_event(
        app,
        "INFO",
        format!(
            "mbus {} -> {}: {}",
            message.from_name,
            if message.target_kind == "topic" {
                format!("#{}", message.topic)
            } else {
                message.target_id.clone()
            },
            preview_text(&message.body, 120)
        ),
    );

    Ok(message)
}

/// Compact status snapshot for prompts/tools. Bus state is cloned before the
/// DB lock so the two locks are never held at the same time.
pub fn status_snapshot(
    app: &AppHandle,
    bus: &MissionBus,
    viewer_id: &str,
    filter: Option<&[String]>,
) -> Value {
    let mut statuses = bus.status();
    if let Some(filter) = filter {
        let wanted: Vec<String> = filter
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect();
        if !wanted.is_empty() {
            statuses.retain(|peer| {
                wanted
                    .iter()
                    .any(|id| id == &peer.id || id == &peer.agent_id)
            });
        }
    }

    let state = app.try_state::<AppState>();
    let now = now_ms();
    let peers: Vec<Value> = statuses
        .into_iter()
        .map(|peer| {
            let mut active_tasks = Vec::new();
            let mut active_task_count = 0usize;
            if peer.state == "running" {
                if let Some(state) = &state {
                    if let Ok(db) = state.db.lock() {
                        if let Ok(tasks) = get_active_mission_tasks(&db, &peer.id) {
                            active_task_count = tasks.len();
                            active_tasks = tasks
                                .into_iter()
                                .take(MAX_STATUS_ACTIVE_TASKS)
                                .map(|task| {
                                    json!({
                                        "task_id": task.task_id,
                                        "name": task.name,
                                        "status": task.status,
                                    })
                                })
                                .collect();
                        }
                    }
                }
            }
            json!({
                "id": peer.id,
                "agent_id": peer.agent_id,
                "agent_name": peer.agent_name,
                "description": peer.description,
                "cwd": peer.cwd,
                "dependencies": peer.dependencies,
                "state": peer.state,
                "can_receive": peer.can_receive,
                "iterations": peer.iterations,
                "tool_calls": peer.tool_calls,
                "tokens": peer.tokens,
                "last_tool": peer.last_tool,
                "idle_ms": peer.last_activity_ms,
                "queued_messages": peer.queued_messages,
                "summary": peer.summary,
                "active_task_count": active_task_count,
                "active_tasks": active_tasks,
            })
        })
        .collect();

    json!({
        "bus_id": bus.bus_id(),
        "closed": bus.is_closed(),
        "now_ms": now,
        "unread_for_viewer": bus.inbox_len(viewer_id),
        "peers": peers,
    })
}

/// Human-readable text version of the status snapshot, used by the planner's
/// automatic activation prompt. The JSON form above is for tool results.
pub fn render_status_text(app: &AppHandle, bus: &MissionBus, viewer_id: &str) -> String {
    let snapshot = status_snapshot(app, bus, viewer_id, None);
    let mut lines: Vec<String> = Vec::new();
    if let Some(peers) = snapshot["peers"].as_array() {
        for peer in peers {
            let id = peer["id"].as_str().unwrap_or("?");
            let name = peer["agent_name"].as_str().unwrap_or("?");
            let state = peer["state"].as_str().unwrap_or("?");
            let rounds = peer["iterations"].as_u64().unwrap_or(0);
            let tool_calls = peer["tool_calls"].as_u64().unwrap_or(0);
            let idle = peer["idle_ms"].as_u64().unwrap_or(0);
            let last_tool = peer["last_tool"].as_str().unwrap_or("-");
            let queued = peer["queued_messages"].as_u64().unwrap_or(0);
            let active = peer["active_task_count"].as_u64().unwrap_or(0);
            let mut line = format!(
                "- {id} [{name}] {state} | rounds: {rounds} | tools: {tool_calls} | last tool: {last_tool} | idle: {}s | queued: {queued}",
                idle / 1000
            );
            if active > 0 {
                line.push_str(&format!(" | active subtasks: {active}"));
            }
            if let Some(summary) = peer["summary"].as_str().filter(|text| !text.is_empty()) {
                line.push_str(&format!(" | result: {}", preview_text(summary, 300)));
            }
            if let Some(deps) = peer["dependencies"].as_array() {
                if !deps.is_empty() {
                    let deps = deps
                        .iter()
                        .filter_map(|dep| dep.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    line.push_str(&format!(" | depends on: {deps}"));
                }
            }
            lines.push(line);
        }
    }
    if lines.is_empty() {
        " (no peers)".to_string()
    } else {
        lines.join("\n")
    }
}

/// Peer roster for the static sub-agent prompt. Tells an agent who it can
/// message and which peers are status-only.
pub fn describe_peers(bus: &MissionBus, self_id: &str) -> String {
    let inner = bus.lock();
    let mut lines = Vec::new();
    for id in &inner.order {
        let Some(peer) = inner.peers.get(id) else {
            continue;
        };
        if peer.peer.id == self_id {
            continue;
        }
        let mut suffix = String::new();
        if peer.peer.is_planner {
            suffix.push_str(" — mission supervisor, receives your messages");
        } else if !peer.peer.can_receive {
            suffix.push_str(" — status only (its runtime cannot call app tools)");
        }
        if !peer.peer.dependencies.is_empty() {
            suffix.push_str(&format!(
                " — depends on {}",
                peer.peer.dependencies.join(", ")
            ));
        }
        lines.push(format!(
            "- {} ({}){}",
            peer.peer.id, peer.peer.agent_name, suffix
        ));
    }
    if lines.is_empty() {
        "(none)".to_string()
    } else {
        lines.join("\n")
    }
}

// ─── Persistence ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
pub struct AgentBusMessageView {
    pub id: i64,
    pub bus_id: String,
    pub from_id: String,
    pub from_name: String,
    pub target_kind: String,
    pub target_id: String,
    pub to_name: String,
    pub topic: String,
    pub body: String,
    pub recipient_count: i64,
    pub created_at: String,
}

pub fn save_bus_message(
    db: &rusqlite::Connection,
    bus: &MissionBus,
    message: &PublishedMessage,
) -> Result<(), String> {
    let to_name = if message.target_kind == "direct" {
        message
            .recipients
            .first()
            .map(|peer| peer.agent_name.clone())
            .unwrap_or_default()
    } else {
        String::new()
    };
    db.execute(
        "INSERT INTO agent_bus_messages \
         (bus_id, session_id, from_id, from_name, target_kind, target_id, to_name, topic, body, recipient_count) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            bus.bus_id(),
            bus.session_id(),
            message.from_id,
            message.from_name,
            message.target_kind,
            message.target_id,
            to_name,
            message.topic,
            message.body,
            message.recipients.len() as i64,
        ],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

pub fn list_bus_messages(
    db: &rusqlite::Connection,
    bus_id: &str,
    limit: usize,
) -> Result<Vec<AgentBusMessageView>, String> {
    let limit = limit.clamp(1, 500) as i64;
    let mut stmt = db
        .prepare(
            "SELECT id, bus_id, from_id, from_name, target_kind, target_id, \
                    to_name, topic, body, recipient_count, COALESCE(created_at, '') \
             FROM agent_bus_messages \
             WHERE bus_id = ?1 \
             ORDER BY id DESC LIMIT ?2",
        )
        .map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![bus_id, limit], |row| {
            Ok(AgentBusMessageView {
                id: row.get(0)?,
                bus_id: row.get(1)?,
                from_id: row.get(2)?,
                from_name: row.get(3)?,
                target_kind: row.get(4)?,
                target_id: row.get(5)?,
                to_name: row.get(6)?,
                topic: row.get(7)?,
                body: row.get(8)?,
                recipient_count: row.get(9)?,
                created_at: row.get(10)?,
            })
        })
        .map_err(|err| err.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())
}

/// Tauri command: recent bus messages for the bus that belongs to `mission_id`.
#[tauri::command]
pub fn list_mission_bus_messages(
    state: tauri::State<'_, AppState>,
    mission_id: String,
    limit: Option<usize>,
) -> Result<Vec<AgentBusMessageView>, String> {
    let db = state.db.lock().map_err(|err| err.to_string())?;
    let bus_id: Option<String> = db
        .query_row(
            "SELECT NULLIF(COALESCE(bus_id, ''), '') FROM agent_missions WHERE mission_id = ?1 LIMIT 1",
            rusqlite::params![mission_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| err.to_string())?
        .flatten();
    let Some(bus_id) = bus_id else {
        return Ok(Vec::new());
    };
    list_bus_messages(&db, &bus_id, limit.unwrap_or(200))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn agent(id: &str, name: &str, kind: &str) -> SubAgent {
        serde_json::from_value(json!({
            "id": id,
            "name": name,
            "description": format!("{name} description"),
            "agent_type": kind,
        }))
        .expect("minimal sub-agent should deserialize")
    }

    fn task(id: &str, agent_id: &str) -> Task {
        serde_json::from_value(json!({
            "id": id,
            "agent_id": agent_id,
            "description": format!("task {id}"),
        }))
        .expect("minimal task should deserialize")
    }

    fn bus() -> MissionBus {
        MissionBus::new(
            "session-1",
            &[task("t1", "a1"), task("t2", "a2")],
            &[agent("a1", "Alpha", "llm"), agent("a2", "Beta", "llm")],
        )
    }

    #[test]
    fn topic_matching_supports_mqtt_wildcards() {
        assert!(topic_matches("broadcast", "broadcast"));
        assert!(topic_matches("build/+", "build/web"));
        assert!(!topic_matches("build/+", "build/web/x"));
        assert!(topic_matches("build/#", "build/web/x"));
        assert!(!topic_matches("build/#", "other/web"));
        assert!(!topic_matches("build/web", "build"));
    }

    #[test]
    fn direct_message_reaches_only_the_target() {
        let bus = bus();
        let published = bus
            .publish("planner", BusTarget::Direct("t1".into()), "hello t1")
            .expect("publish should succeed");
        assert_eq!(published.recipients.len(), 1);
        assert_eq!(bus.take_inbox("t1", 8).len(), 1);
        assert!(bus.take_inbox("t2", 8).is_empty());
    }

    #[test]
    fn broadcast_reaches_every_receivable_peer_but_not_the_sender() {
        let bus = bus();
        let published = bus
            .publish("t1", BusTarget::Topic("broadcast".into()), "all hands")
            .expect("broadcast should succeed");
        let ids: Vec<&str> = published
            .recipients
            .iter()
            .map(|peer| peer.id.as_str())
            .collect();
        assert_eq!(ids, vec!["planner", "t2"]);
        assert!(bus.take_inbox("t1", 8).is_empty());
        assert_eq!(bus.take_inbox("t2", 8).len(), 1);
        assert_eq!(bus.take_inbox(PLANNER_ID, 8).len(), 1);
    }

    #[test]
    fn topic_subscription_filters_delivery() {
        let bus = bus();
        bus.subscribe("t2", &["build/#".to_string()])
            .expect("subscribe should succeed");
        let published = bus
            .publish("t1", BusTarget::Topic("build/web".into()), "web built")
            .expect("topic publish should succeed");
        assert_eq!(published.recipients.len(), 1);
        assert_eq!(published.recipients[0].id, "t2");
        assert_eq!(bus.take_inbox("t2", 8).len(), 1);
    }

    #[test]
    fn terminal_peer_rejects_new_messages() {
        let bus = bus();
        bus.finish_task(&TaskResult {
            task_id: "t1".into(),
            agent_id: "a1".into(),
            agent_name: "Alpha".into(),
            status: "success".into(),
            content: "done".into(),
            tool_calls_count: 1,
            tokens_used: 2,
        });
        let err = bus
            .publish("planner", BusTarget::Direct("t1".into()), "late")
            .expect_err("terminal peer must reject");
        assert!(err.contains("already finished"));
    }

    #[test]
    fn inbox_drain_is_limited_and_preserves_the_rest() {
        let bus = bus();
        for index in 0..10 {
            bus.publish(
                "planner",
                BusTarget::Direct("t1".into()),
                &format!("m{index}"),
            )
            .expect("publish should succeed");
        }
        let first = bus.take_inbox("t1", 4);
        assert_eq!(first.len(), 4);
        assert_eq!(first[0].body, "m0");
        assert_eq!(bus.inbox_len("t1"), 6);
        let rest = bus.take_inbox("t1", 8);
        assert_eq!(rest.len(), 6);
        assert_eq!(rest[5].body, "m9");
    }

    #[test]
    fn body_and_message_limits_are_enforced() {
        let bus = bus();
        let long = "x".repeat(MAX_BODY_CHARS + 1);
        assert!(bus
            .publish("planner", BusTarget::Direct("t1".into()), &long)
            .is_err());

        for _ in 0..MAX_MESSAGES_PER_PEER {
            bus.publish("t1", BusTarget::Direct("t2".into()), "ok")
                .expect("publish should succeed");
            bus.take_inbox("t2", 8);
        }
        assert!(bus
            .publish("t1", BusTarget::Direct("t2".into()), "too many")
            .is_err());
    }

    #[test]
    fn status_reports_state_rounds_and_summary() {
        let bus = bus();
        bus.mark_running("t1");
        bus.mark_iteration("t1", 3, 120);
        bus.mark_tool("t1", "run_cmd");
        bus.finish_task(&TaskResult {
            task_id: "t2".into(),
            agent_id: "a2".into(),
            agent_name: "Beta".into(),
            status: "success".into(),
            content: "research complete".into(),
            tool_calls_count: 2,
            tokens_used: 40,
        });
        let statuses = bus.status();
        let t1 = statuses.iter().find(|peer| peer.id == "t1").unwrap();
        assert_eq!(t1.state, "running");
        assert_eq!(t1.iterations, 3);
        assert_eq!(t1.last_tool.as_deref(), Some("run_cmd"));
        let t2 = statuses.iter().find(|peer| peer.id == "t2").unwrap();
        assert_eq!(t2.state, "completed");
        assert_eq!(t2.summary, "research complete");
    }

    #[test]
    fn history_is_scoped_for_agents_but_global_for_the_planner() {
        let bus = bus();
        bus.publish("t1", BusTarget::Direct("t2".into()), "private note")
            .expect("publish should succeed");
        bus.publish("planner", BusTarget::Direct("t1".into()), "planner note")
            .expect("publish should succeed");
        assert_eq!(bus.history("t1", None, 20).len(), 2);
        assert_eq!(bus.history("t2", None, 20).len(), 1);
        assert_eq!(bus.history(PLANNER_ID, None, 20).len(), 2);
    }
}
