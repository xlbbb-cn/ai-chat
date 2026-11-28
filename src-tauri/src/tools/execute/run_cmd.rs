//! `run_cmd`: the script-library / PATH-program tool.

use super::*;

pub(super) async fn handle(ctx: &ToolContext<'_>, args: &Value) -> String {
    let app = ctx.app;
    let workspace_dir = ctx.workspace_dir.to_path_buf();
    let allowed_commands = ctx.allowed_commands;
    let mission_id = ctx.mission_id;
    let session_id = ctx.session_id;
    let call_id = ctx.call_id;
    let working_dir = ctx.working_dir;
    let cancel = ctx.cancel;
    let action = args["action"]
        .as_str()
        .unwrap_or("run")
        .trim()
        .to_ascii_lowercase();

    // ── Script library listing ────────────────────────────────────────
    if action == "list" {
        return scripts::render_list(&workspace_dir);
    }
    if action != "run" {
        return format!(
            "Error: unsupported run_cmd action '{}'. Supported actions: run, list",
            action
        );
    }

    let command = args["command"].as_str().unwrap_or("").trim().to_string();
    let command_cwd = working_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| workspace_dir.clone());

    if command.is_empty() {
        return "Error: run_cmd requires a non-empty 'command' argument when action is 'run'."
            .to_string();
    }

    // Split once: the first token addresses either the workspace script
    // library or a PATH-resolved program; the rest are its arguments.
    let parts = split_command_line(&command);
    let program = parts.first().map(|s| s.as_str()).unwrap_or("");

    // ── PATH-resolved program only ───────────────────────────────────
    // `run_cmd` runs a bare name the OS resolves through `PATH`
    // (`python`, `python3`, `git`, …) or a library script name.
    // Absolute and relative paths are refused — `run_shell` is the tool
    // that may run any script, from anywhere, under the risk audit.
    if let Err(e) = process::validate_path_resolved_program(program) {
        return e;
    }

    let scripts_dir = crate::workspace::scripts_root(&workspace_dir);
    // The host environment rides along in every `run_cmd` child: `<env root>/bin`
    // and the venv's `bin/` join the child's `PATH` right behind the script
    // library, and `VIRTUAL_ENV` names the venv — so a tool the assistant
    // installed in the environment root runs by bare name, and `python` /
    // `python3` (including the interpreter of a `.py` library script) is the
    // 3.13 one. `run_shell` keeps the host interpreter on purpose.
    let data_dir = crate::env::data_dir_from_app(app);
    let env_dirs = data_dir
        .as_deref()
        .map(crate::env::tool_path_dirs)
        .unwrap_or_default();
    let mounted_venv = data_dir.as_deref().and_then(crate::env::mounted_venv);
    let resolved = match scripts::resolve(&workspace_dir, program) {
        Ok(resolved) => resolved,
        Err(err) => return format!("Error: {err}"),
    };

    // ── No shell interpreters as PATH programs ────────────────────────
    // `bash script.sh`, `cmd /c …`, `powershell -File …` would hand
    // free-form code to an interpreter under run_cmd's "direct" risk
    // assessment, bypassing the script audit `run_shell` applies. A
    // resolved library script is the exception: its content is scanned
    // and approved below, so `bash` as the interpreter of `bash.sh`
    // (or any `.sh` library script) stays legal.
    if resolved.is_none() {
        if let Err(e) = process::reject_shell_interpreter(program) {
            return e;
        }
    }

    // ── Allowed-commands enforcement (skill context) ──────────────────
    // A library script is allowed by its command name, its file name or
    // the interpreter that runs it.
    if !allowed_commands.is_empty() {
        let exe_name = Path::new(program)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(program)
            .to_lowercase();
        let mut permitted = allowed_commands
            .iter()
            .any(|a| a.eq_ignore_ascii_case(&exe_name));
        if let Some(script) = &resolved {
            permitted = permitted
                || allowed_commands
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(&script.entry.name))
                || allowed_commands
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(&script.entry.file_name))
                || allowed_commands
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(script.entry.interpreter.program_name()));
        }
        if !permitted {
            return format!(
                "Error: Command '{}' is not in this skill's allowed-commands list ({}).",
                exe_name,
                allowed_commands.join(", ")
            );
        }
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
    let report = generate_risk_report("direct", &command, &request_id);

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
                "cmd_type": "direct",
                "code": command,
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

    // ── Script body safety scan ───────────────────────────────────────
    // The command line cannot show what a script does. Scan the resolved
    // script and every script passed to an interpreter as an argument.
    // A clean scan needs no approval; a hit is approved once per exact
    // content hash and re-prompts after the file changes.
    let mut scan_targets: Vec<PathBuf> = Vec::new();
    if let Some(script) = &resolved {
        scan_targets.push(script.path.clone());
    }
    for path in scripts::referenced_scripts(&workspace_dir, &command_cwd, &parts[1..]) {
        if !scan_targets.contains(&path) {
            scan_targets.push(path);
        }
    }

    let mut scans: Vec<scripts::ScriptScan> = Vec::new();
    for path in &scan_targets {
        match scripts::scan(path) {
            Ok(scan) => scans.push(scan),
            Err(err) => return format!("Error: Script safety scan failed: {err}"),
        }
    }

    let unapproved: Vec<&scripts::ScriptScan> = scans
        .iter()
        .filter(|scan| {
            !scan.hits.is_empty()
                && !scripts::is_approved(&workspace_dir, &scan.file_name, &scan.hash)
        })
        .collect();

    let script_scan_hits: Vec<Value> = scans
        .iter()
        .filter(|scan| !scan.hits.is_empty())
        .map(|scan| {
            json!({
                "file": scan.file_name,
                "hash": scan.hash,
                "approved": scripts::is_approved(
                    &workspace_dir,
                    &scan.file_name,
                    &scan.hash,
                ),
                "hits": scan.hits,
            })
        })
        .collect();
    let script_label = resolved
        .as_ref()
        .map(|script| script.entry.file_name.clone());

    // ── Emit risk assessment event for frontend display ───────────────
    let _ = app.emit(
        crate::events::RISK_ASSESSMENT,
        serde_json::json!({
            "request_id": request_id,
            "cmd_type": "direct",
            "code": command,
            "risk_level": report.risk_level,
            "risk_score": report.risk_score,
            "disposition": report.disposition,
            "recommendation": report.recommendation,
            "blacklist_hits": report.blacklist_hits,
            "penalty_items": report.penalty_items,
            "requires_confirmation": !risk_level.is_auto_approvable()
                || !unapproved.is_empty(),
            "script": script_label,
            "script_scan_hits": script_scan_hits,
            "report": &report,
        }),
    );

    let needs_risk_confirmation = !risk_level.is_auto_approvable();
    let timeout_secs = args["timeout_seconds"]
        .as_i64()
        .unwrap_or(30)
        .clamp(1, 3600) as u64;

    // ── L5/L6 + clean script: run directly ────────────────────────────
    if !needs_risk_confirmation && unapproved.is_empty() {
        let _ = emit_tool_progress(
            app,
            mission_id,
            session_id,
            call_id,
            format!(
                "*Risk assessment passed: {} (score: {}) - running directly*\n\n```\n{}\n```\n\n",
                report.risk_level, report.risk_score, command
            ),
        );

        return run_run_cmd(
            &workspace_dir,
            &scripts_dir,
            resolved.as_ref(),
            &parts,
            &command,
            command_cwd,
            timeout_secs,
            cancel.cloned(),
            &env_dirs,
            mounted_venv.as_deref(),
        )
        .await;
    }

    // ── One dialog for the command risk and the script content ────────
    // Both checks describe the same execution; asking twice for one run
    // would be noise. The script findings are included only when this
    // exact content was never approved.
    let mut reason = String::new();
    if needs_risk_confirmation {
        reason.push_str(&format!(
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
        ));
    }
    if !unapproved.is_empty() {
        if !reason.is_empty() {
            reason.push_str("\n\n");
        }
        reason.push_str(
            "Script safety scan: the script body contains destructive or sensitive patterns. \
             Approving remembers this exact content until the file changes.\n",
        );
        for scan in &unapproved {
            reason.push_str(&format!("\n- {} ({})\n", scan.file_name, scan.hash));
            for hit in &scan.hits {
                reason.push_str(&format!(
                    "  - [{}] {}: {}\n",
                    hit.rule_id, hit.severity, hit.matched
                ));
            }
        }
    }

    // A script hit is always the `dangerous` kind: the body scan only
    // fires on destructive or sensitive patterns, so a user who
    // auto-accepts a weaker kind must still see this one.
    let confirm_kind = if unapproved.is_empty() {
        risk_level.confirm_kind()
    } else {
        "dangerous"
    };
    let confirm = request_tool_confirmation(
        app,
        session_id,
        reason,
        "direct".to_string(),
        command.clone(),
        confirm_kind,
        "none",
        cancel,
        call_id,
    )
    .await;

    if !confirm.confirmed {
        let mut out = String::from("Error: Execution rejected by user");
        if needs_risk_confirmation {
            out.push_str(&format!(
                "\n\nRisk level: {}\nRisk score: {}\nDisposition: {}\n\nStructured report:\n```json\n{}\n```",
                report.risk_level,
                report.risk_score,
                report.disposition,
                serde_json::to_string_pretty(&report).unwrap_or_default()
            ));
        }
        if !unapproved.is_empty() {
            out.push_str(&format!(
                "\n\nUnapproved script safety findings: {}",
                unapproved
                    .iter()
                    .map(|scan| scan.file_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        return out;
    }

    // The user accepted exactly this content: remember it per file.
    for scan in &unapproved {
        scripts::approve(&workspace_dir, &scan.file_name, &scan.hash);
    }

    let _ = emit_tool_progress(
        app,
        mission_id,
        session_id,
        call_id,
        if needs_risk_confirmation {
            format!(
                "*User confirmed execution: {} (score: {})*\n\n```\n{}\n```\n\n",
                report.risk_level, report.risk_score, command
            )
        } else {
            format!(
                "*User approved script content: {}*\n\n```\n{}\n```\n\n",
                unapproved
                    .iter()
                    .map(|scan| scan.file_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                command
            )
        },
    );

    run_run_cmd(
        &workspace_dir,
        &scripts_dir,
        resolved.as_ref(),
        &parts,
        &command,
        command_cwd,
        timeout_secs,
        cancel.cloned(),
        &env_dirs,
        mounted_venv.as_deref(),
    )
    .await
}

/// Execute one `run_cmd` invocation after every gate (syntax, risk, script
/// safety) has passed.
///
/// A library script runs as interpreter + path + arguments, so the path is one
/// `Command` argument and never re-parsed as shell syntax. Anything else goes
/// through the historical `direct` command path. Both get the same
/// [`process::Mounts`]: the script library and, when this host has one, the
/// app-data environment (its `bin/` and the 3.13 venv).
#[allow(clippy::too_many_arguments)]
async fn run_run_cmd(
    workspace_dir: &Path,
    scripts_dir: &Path,
    resolved: Option<&scripts::ResolvedScript>,
    parts: &[String],
    command: &str,
    cwd: PathBuf,
    timeout_secs: u64,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    env_dirs: &[std::path::PathBuf],
    venv: Option<&Path>,
) -> String {
    let mounts = process::Mounts {
        scripts: Some(scripts_dir),
        env_dirs,
        venv,
    };
    match resolved {
        Some(script) => {
            let (program, args) = script.command_parts(&parts[1..]);
            let output = process::run_program_timed(
                program,
                args,
                Some(cwd),
                timeout_secs,
                cancel,
                mounts,
            )
            .await;
            scripts::record_use(workspace_dir, &script.entry.file_name, &output);
            output
        }
        None => {
            process::run_command_timed(
                "direct".to_string(),
                command.to_string(),
                Some(cwd),
                timeout_secs,
                cancel,
                mounts,
            )
            .await
        }
    }
}
