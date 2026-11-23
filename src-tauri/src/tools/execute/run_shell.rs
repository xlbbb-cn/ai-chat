//! `run_shell`: the audited free-form shell tool (direct + serve).

use super::*;

pub(super) async fn handle(ctx: &ToolContext<'_>, args: &Value) -> String {
    let app = ctx.app;
    let workspace_dir = ctx.workspace_dir.to_path_buf();
    let mission_id = ctx.mission_id;
    let session_id = ctx.session_id;
    let call_id = ctx.call_id;
    let working_dir = ctx.working_dir;
    let cancel = ctx.cancel;
    // The interpreter is a build-time property of this desktop app
    // (Windows → pwsh, Linux/macOS → bash). A `type` supplied by an old
    // prompt or a reused tool call is ignored: the model writes the
    // script, the build decides what runs it.
    let shell_type = RUN_SHELL_TYPE.to_string();
    let run_type = args["run_type"]
        .as_str()
        .unwrap_or("direct")
        .trim()
        .to_ascii_lowercase();
    let action = args["action"]
        .as_str()
        .unwrap_or("start")
        .trim()
        .to_ascii_lowercase();
    let command_cwd = working_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| workspace_dir.clone());

    if !matches!(run_type.as_str(), "direct" | "serve") {
        return format!(
            "Error: unsupported run_type '{}'. Supported: direct, serve",
            run_type
        );
    }

    // Serve management is about processes this app started, not about
    // running a script — no shell validation, risk assessment or
    // confirmation is needed to list/stop them.
    if run_type == "serve" {
        match action.as_str() {
            "list" => {
                return app.state::<crate::AppState>().serve_processes.list();
            }
            "stop" => {
                let serve_id = args["serve_id"]
                    .as_str()
                    .or_else(|| args["code"].as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                return app
                    .state::<crate::AppState>()
                    .serve_processes
                    .stop(&serve_id);
            }
            "start" => {}
            other => {
                return format!(
                    "Error: unsupported serve action '{}'. Supported: start, stop, list",
                    other
                );
            }
        }
    } else if action != "start" {
        return format!(
            "Error: action '{}' is only valid with run_type 'serve'. Use run_type 'direct' to run a script.",
            action
        );
    }

    let code = args["code"].as_str().unwrap_or("").to_string();

    if code.trim().is_empty() {
        return "Error: run_shell requires a non-empty 'code' argument.".to_string();
    }

    // ── Patent-compliant risk assessment ──────────────────────────────
    let request_id = format!(
        "req-{}",
        uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("unknown")
    );
    let report = generate_risk_report(&shell_type, &code, &request_id);

    // ── Syntax Gate: reject invalid scripts before execution ─────────
    if report.syntax_check.status == "failed" {
        let error_details: Vec<String> = report
            .syntax_check
            .syntax_errors
            .iter()
            .map(|e| {
                format!(
                    "  - Line {}:{}: {} ({})",
                    e.line, e.column, e.message, e.error_type
                )
            })
            .collect();

        // Emit syntax error event for frontend
        let _ = app.emit(
            crate::events::SYNTAX_ERROR,
            serde_json::json!({
                "request_id": request_id,
                "cmd_type": shell_type,
                "code": code,
                "syntax_errors": report.syntax_check.syntax_errors,
                "report": report,
            }),
        );

        return format!(
            "Error: Syntax gate blocked: the script is syntactically invalid and will not run\n\n\
            Error details:\n{}\n\n\
            Fix suggestions:\n{}\n\n\
            Structured report:\n```json\n{}\n```",
            error_details.join("\n"),
            report
                .syntax_check
                .syntax_errors
                .iter()
                .map(|e| format!("  - {}", e.suggestion))
                .collect::<Vec<_>>()
                .join("\n"),
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
    }

    let risk_level = score_to_risk_level(report.risk_score);
    let scoped_code = build_workspace_scoped_shell_code(&shell_type, &command_cwd, &code);

    // ── Emit risk assessment event for frontend display ───────────────
    let _ = app.emit(
        crate::events::RISK_ASSESSMENT,
        serde_json::json!({
            "request_id": request_id,
            "cmd_type": shell_type,
            "code": code,
            "risk_level": report.risk_level,
            "risk_score": report.risk_score,
            "disposition": report.disposition,
            "recommendation": report.recommendation,
            "blacklist_hits": report.blacklist_hits,
            "penalty_items": report.penalty_items,
            "requires_confirmation": !risk_level.is_auto_approvable(),
            "report": report,
        }),
    );

    // ── L5/L6: run directly (no confirmation) ────────────────────────
    if risk_level.is_auto_approvable() {
        let _ = emit_tool_progress(
            app,
            mission_id,
            session_id,
            call_id,
            format!(
                "*Risk assessment passed: {} (score: {}) - running directly*\n\n```{}\n{}\n```\n\n",
                report.risk_level, report.risk_score, shell_type, code
            ),
        );

        let timeout_secs = args["timeout_seconds"]
            .as_i64()
            .unwrap_or(30)
            .clamp(1, 3600) as u64;

        if run_type == "serve" {
            return serve::start(app, &shell_type, &code, scoped_code, command_cwd, session_id.unwrap_or("")).await;
        }

        return process::run_command_timed(
            shell_type,
            scoped_code,
            Some(command_cwd),
            timeout_secs,
            cancel.cloned(),
            None,
        )
        .await;
    }

    // ── L0-L3: require frontend confirmation (L4-L6 were auto-approved) ──
    let confirm = request_tool_confirmation(
        app,
        format!(
            "Risk assessment: {} (score: {})\nDisposition: {}\n\nMatched rules:\n{}\n\nPenalty items:\n{}",
            report.risk_level,
            report.risk_score,
            report.disposition,
            report.blacklist_hits.iter()
                .map(|h| format!("  - [{}] {}: {}", h.rule_id, h.severity, h.matched))
                .collect::<Vec<_>>()
                .join("\n"),
            report.penalty_items.iter()
                .map(|p| format!("  - {}: +{}", p.name, p.points))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        shell_type.clone(),
        code.clone(),
        risk_level.confirm_kind(),
        "none",
        cancel,
        call_id,
    )
    .await;

    if !confirm.confirmed {
        return format!(
            "Error: Execution rejected by user\n\n\
            Risk level: {}\n\
            Risk score: {}\n\
            Disposition: {}\n\n\
            Structured report:\n```json\n{}\n```",
            report.risk_level,
            report.risk_score,
            report.disposition,
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
    }

    // ── Execute after user confirmation ───────────────────────────────
    let _ = emit_tool_progress(
        app,
        mission_id,
        session_id,
        call_id,
        format!(
            "*User confirmed execution: {} (score: {})*\n\n```{}\n{}\n```\n\n",
            report.risk_level, report.risk_score, shell_type, code
        ),
    );

    let timeout_secs = args["timeout_seconds"]
        .as_i64()
        .unwrap_or(30)
        .clamp(1, 3600) as u64;

    if run_type == "serve" {
        return serve::start(app, &shell_type, &code, scoped_code, command_cwd, session_id.unwrap_or("")).await;
    }

    process::run_command_timed(
        shell_type,
        scoped_code,
        Some(command_cwd),
        timeout_secs,
        cancel.cloned(),
        None,
    )
    .await
}
