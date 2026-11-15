//! Which tool calls of one assistant round may run concurrently.
//!
//! A round is the list of `tool_calls` the model emitted in one response. Most
//! of them are independent reads — file reads, searches, builds, MCP queries —
//! but a few mutate state the *round* shares: `use_skill` grows the allowed
//! command list and the skill directories the rest of the round is executed
//! against, and `file_actions` write/patch/delete rewrite a file another call
//! in the same round may be reading. Those run alone, in `tool_calls` order,
//! and split the round into segments.
//!
//! Classification is deliberately an allowlist: a tool nobody thought about
//! here (or a future one) is a barrier, which is exactly the sequential
//! behaviour that existed before batching. Silence is safe.

use serde_json::Value;

/// How one tool call is scheduled inside its round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallMode {
    /// Runs alone, in `tool_calls` order, and splits the concurrent segments
    /// on both sides of it.
    Barrier,
    /// May run together with the concurrent calls next to it.
    Concurrent,
}

/// `file_actions` actions that only read the workspace. Every other action of
/// that tool (`write`, `patch`, `rename`, `move`, `mkdir`, `delete`) rewrites
/// state a sibling call could be reading, so those are barriers.
const READ_ONLY_FILE_ACTIONS: &[&str] = &["read", "list", "search", "diff"];

/// Whether `name` may share its segment with a neighbouring call.
///
/// `is_mcp` is decided by the caller: MCP tool names are generated per server
/// (`mcp_<hash>_<tool>`) and are not in this match at all. They are safe to
/// interleave because a stdio server serialises its own pool and the HTTP
/// transports open one connection per call.
pub(crate) fn call_mode(name: &str, args: &Value, is_mcp: bool) -> CallMode {
    if is_mcp {
        return CallMode::Concurrent;
    }
    match name {
        // One process per call; the runner awaits the child, so two of them
        // overlap instead of queueing. The shell working directory is the
        // caller's, and two commands that fight over it are the model's
        // choice, not a race this module can resolve.
        "run_cmd" | "run_shell" => CallMode::Concurrent,
        "file_actions" => {
            let action = args["action"].as_str().unwrap_or("");
            if READ_ONLY_FILE_ACTIONS.contains(&action) {
                CallMode::Concurrent
            } else {
                CallMode::Barrier
            }
        }
        // Read-only within one skill directory.
        "skill_read" => CallMode::Concurrent,
        // A delegation owns its own mission, MCP session and deliverable
        // directory; two delegations in one round are independent.
        "call_subagent" => CallMode::Concurrent,
        // Everything else mutates round state (`use_skill`), a per-session
        // file another call may read-modify-write (`memory`, `todos`), the
        // timer registry, the mission database or the tool registry
        // (`background_task`, `mcp_self`, `skill_merge`, `experience`).
        _ => CallMode::Barrier,
    }
}

/// Splits a round into the segments it is executed in, as half-open index
/// ranges. A barrier is always a one-element range, so the caller can execute
/// every range in order: a range of one runs alone, a longer range may run its
/// calls concurrently.
pub(crate) fn plan_segments(modes: &[CallMode]) -> Vec<std::ops::Range<usize>> {
    let mut segments = Vec::new();
    let mut start = 0;
    while start < modes.len() {
        if modes[start] == CallMode::Barrier {
            segments.push(start..start + 1);
            start += 1;
            continue;
        }
        let mut end = start;
        while end < modes.len() && modes[end] == CallMode::Concurrent {
            end += 1;
        }
        segments.push(start..end);
        start = end;
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mode(name: &str, args: Value) -> CallMode {
        call_mode(name, &args, false)
    }

    #[test]
    fn reads_search_and_builds_run_concurrently() {
        assert_eq!(
            mode("file_actions", json!({"action": "read"})),
            CallMode::Concurrent
        );
        assert_eq!(
            mode("file_actions", json!({"action": "search"})),
            CallMode::Concurrent
        );
        assert_eq!(mode("run_cmd", json!({})), CallMode::Concurrent);
        assert_eq!(mode("run_shell", json!({})), CallMode::Concurrent);
        assert_eq!(mode("skill_read", json!({})), CallMode::Concurrent);
        assert_eq!(mode("call_subagent", json!({})), CallMode::Concurrent);
    }

    #[test]
    fn mutations_and_round_state_are_barriers() {
        for action in ["write", "patch", "rename", "move", "mkdir", "delete"] {
            assert_eq!(
                mode("file_actions", json!({"action": action})),
                CallMode::Barrier,
                "file_actions {action} must not run next to a sibling"
            );
        }
        for name in [
            "use_skill",
            "memory",
            "todos",
            "timer_set",
            "timer_list",
            "timer_cancel",
            "background_task",
            "mcp_self",
            "skill_merge",
            "experience",
            "add_task",
            "update_task_status",
            "get_active_tasks",
            "mark_mission_accomplished",
        ] {
            assert_eq!(mode(name, json!({})), CallMode::Barrier, "{name}");
        }
        // An unknown tool is a barrier: silence must stay sequential.
        assert_eq!(mode("some_future_tool", json!({})), CallMode::Barrier);
    }

    #[test]
    fn mcp_tools_are_concurrent() {
        assert_eq!(
            call_mode("mcp_deadbeef_search", &json!({}), true),
            CallMode::Concurrent
        );
        // The same name without the MCP map entry is unknown, hence a barrier.
        assert_eq!(
            call_mode("mcp_deadbeef_search", &json!({}), false),
            CallMode::Barrier
        );
    }

    #[test]
    fn segments_split_on_barriers() {
        use CallMode::{Barrier, Concurrent};
        assert_eq!(
            plan_segments(&[Concurrent, Concurrent, Barrier, Concurrent]),
            vec![0..2, 2..3, 3..4]
        );
        assert_eq!(
            plan_segments(&[Barrier, Concurrent, Concurrent, Barrier, Barrier]),
            vec![0..1, 1..3, 3..4, 4..5]
        );
        assert_eq!(plan_segments(&[Barrier]), vec![0..1]);
        assert!(plan_segments(&[]).is_empty());
    }
}
