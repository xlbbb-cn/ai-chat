// Risk classification for tool commands: L0-L6 seven-level rating, syntax
// gate, weighted risk score and the structured assessment report.

use std::path::Path;

use super::split_command_line;

// ─── Command risk classification ─────────────────────────────────────────────

/// Risk level assigned to a command before execution.
/// Lower numeric value = higher risk (L0 is most dangerous).
/// L0-L6 seven-level risk rating per patent requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// L0 — System risk: could modify OS files, boot config, kernel, or system services.
    /// 85 ≤ score ≤ 100: behavior that destroys underlying system files and causes downtime
    L0 = 0,
    /// L1 — System config risk: modify system config files and system software.
    /// 70 ≤ score < 85: behavior that modifies system config files and system software
    L1 = 1,
    /// L2 — User software risk: install/uninstall/modify user applications.
    /// 55 ≤ score < 70: behavior that modifies user software and configuration
    L2 = 2,
    /// L3 — User data risk: modify/delete user data files.
    /// 40 ≤ score < 55: behavior that modifies user data
    L3 = 3,
    /// L4 — Sensitive read: read system/user configs, keys, logs.
    /// 25 ≤ score < 40: behavior that reads sensitive information
    L4 = 4,
    /// L5 — General query: no side effects status/info queries.
    /// 10 ≤ score < 25: general query behavior
    L5 = 5,
    /// L6 — Safe: read-only, informational, or purely non-destructive.
    /// 0 ≤ score < 10: read-only behavior
    L6 = 6,
}

impl RiskLevel {
    pub fn description(self) -> &'static str {
        match self {
            RiskLevel::L0 => "L0 system risk — may affect OS integrity (destroy underlying system files)",
            RiskLevel::L1 => "L1 system config risk — may modify system config/software (modify system configuration)",
            RiskLevel::L2 => "L2 user software risk — may modify user applications (modify user software)",
            RiskLevel::L3 => "L3 user data risk — may modify or delete user files (modify or delete user data)",
            RiskLevel::L4 => "L4 sensitive read — may read sensitive info (read sensitive information)",
            RiskLevel::L5 => "L5 general query — read-only status/info (general query)",
            RiskLevel::L6 => "L6 safe — read-only or non-destructive (read-only behavior)",
        }
    }

    /// Confirmation `kind` string sent to the frontend; also used for auto-accept config.
    pub fn confirm_kind(self) -> &'static str {
        match self {
            RiskLevel::L0 => "dangerous",
            RiskLevel::L1 => "system_config",
            RiskLevel::L2 => "user_software",
            RiskLevel::L3 => "user_data",
            RiskLevel::L4 => "sensitive_read",
            RiskLevel::L5 => "general_query",
            RiskLevel::L6 => "safe",
        }
    }

    /// Get disposition suggestion per patent requirements.
    /// L0/L1: manual review, L2/L3: LLM warning + confirmation, L4: allow + audit, L5: allow, L6: allow directly
    pub fn disposition(self) -> &'static str {
        match self {
            RiskLevel::L0 => "Manual review (force reject if score ≥ 90)",
            RiskLevel::L1 => "Manual review",
            RiskLevel::L2 => "LLM warning and confirm execution",
            RiskLevel::L3 => "LLM warning and confirm execution",
            RiskLevel::L4 => "Recommend to allow and log audit",
            RiskLevel::L5 => "Recommend to allow",
            RiskLevel::L6 => "Recommend to allow directly",
        }
    }

    /// Check if this level requires human review (L0/L1).
    pub fn requires_human_review(self) -> bool {
        matches!(self, RiskLevel::L0 | RiskLevel::L1)
    }

    /// Check if this level requires LLM confirmation (L2/L3).
    pub fn requires_llm_confirmation(self) -> bool {
        matches!(self, RiskLevel::L2 | RiskLevel::L3)
    }

    /// Check if this level should be auto-approved (L4-L6).
    pub fn is_auto_approvable(self) -> bool {
        matches!(self, RiskLevel::L4 | RiskLevel::L5 | RiskLevel::L6)
    }
}

/// Risk score thresholds for L0-L6 mapping (per patent: 10/25/40/55/70/85).
pub const RISK_THRESHOLDS: [u32; 6] = [10, 25, 40, 55, 70, 85];

/// Reject threshold - scores >= this trigger forced rejection.
pub const REJECT_THRESHOLD: u32 = 90;

/// Map risk score (0-100) to L0-L6 level.
pub fn score_to_risk_level(score: u32) -> RiskLevel {
    let score = score.min(100);
    if score >= RISK_THRESHOLDS[5] {
        RiskLevel::L0
    } else if score >= RISK_THRESHOLDS[4] {
        RiskLevel::L1
    } else if score >= RISK_THRESHOLDS[3] {
        RiskLevel::L2
    } else if score >= RISK_THRESHOLDS[2] {
        RiskLevel::L3
    } else if score >= RISK_THRESHOLDS[1] {
        RiskLevel::L4
    } else if score >= RISK_THRESHOLDS[0] {
        RiskLevel::L5
    } else {
        RiskLevel::L6
    }
}

/// Structured risk assessment report per patent requirements.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RiskAssessmentReport {
    pub request_id: String,
    pub risk_level: String,
    pub risk_score: u32,
    pub disposition: String,
    pub dimension_scores: DimensionScores,
    pub blacklist_hits: Vec<BlacklistHit>,
    pub penalty_items: Vec<PenaltyItem>,
    pub whitelist_coverage: WhitelistCoverage,
    pub semantic_anomalies: Vec<String>,
    pub recommendation: String,
    pub syntax_check: SyntaxCheckResult,
    pub timestamp: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DimensionScores {
    pub blacklist_severity: f32,
    pub whitelist_coverage: f32,
    pub semantic_anomaly: f32,
    pub dangerous_capability: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BlacklistHit {
    pub rule_id: String,
    pub severity: String,
    pub matched: String,
    pub contribution: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PenaltyItem {
    pub name: String,
    pub points: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WhitelistCoverage {
    pub covered: usize,
    pub total: usize,
    pub ratio: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SyntaxCheckResult {
    pub status: String, // "passed" or "failed"
    pub syntax_errors: Vec<SyntaxError>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SyntaxError {
    pub line: usize,
    pub column: usize,
    pub error_type: String,
    pub message: String,
    pub suggestion: String,
}

// ─── Sorted executable name sets (binary_search, O(log n)) ───────────────────

/// L6 executables — known-safe, read-only or informational.
static L6_EXECUTABLES: &[&str] = &[
    "alias", "arch", "awk", "basename", "cat", "cmp", "comm", "curl", "cut", "date", "df", "diff",
    "dirname", "du", "echo", "env", "file", "find", "free", "grep", "head", "help", "history",
    "hostname", "htop", "info", "less", "ln", "locate", "ls", "man", "more", "printenv", "printf",
    "ps", "pwd", "readlink", "realpath", "sed", "sort", "stat", "tail", "time", "top", "tr",
    "type", "uname", "uniq", "uptime", "wc", "wget", "whatis", "whereis", "which", "whoami",
];

// ─── System path prefixes (escalate to L0 if targeted by a destructive cmd) ──

static SYSTEM_PATH_PREFIXES: &[&str] = &[
    "/etc/",
    "/boot/",
    "/sys/",
    "/proc/",
    "/dev/",
    "/usr/lib/",
    "/usr/share/",
    "/usr/local/lib/",
    "/lib/",
    "/lib64/",
    "/bin/",
    "/sbin/",
    "/usr/bin/",
    "/usr/sbin/",
    "/System/",
    "/Library/",
    "/Applications/",
    "c:\\windows\\",
    "c:\\program files\\",
    "c:\\program files (x86)\\",
    "c:\\programdata\\",
    "hkey_local_machine",
    "hklm\\",
];

// ─── Compiled regex sets (built once, matched in a single pass) ──────────────

fn l0_regex_set() -> &'static regex::RegexSet {
    use std::sync::OnceLock;
    static SET: OnceLock<regex::RegexSet> = OnceLock::new();
    SET.get_or_init(|| {
        regex::RegexSet::new([
            // Disk destruction
            r"dd\s+if=.*of=/dev/",
            r"mkfs\.",
            r">\s*/dev/sd",
            r">\s*/dev/nvme",
            r">\s*/dev/xvd",
            r">\s*/dev/mmcblk",
            // System services
            r"systemctl\s+(stop|disable|mask)\s",
            r"sc\s+(stop|delete|config)\s",
            // Boot/OS config
            r"bcdedit\s+/",
            r"grub-install",
            r"update-grub",
            // Security bypass
            r"set-executionpolicy\s+(unrestricted|bypass|remotesigned)",
            r"invoke-expression\s",
            r"\biex\s",
            // Firewall
            r"iptables\s+-[ADIF]",
            r"nft\s+(add|delete|insert)\s",
            r"ufw\s+(enable|disable|deny|allow)",
            // Kernel modules
            r"(insmod|modprobe|rmmod)\s",
            // Shutdown/reboot
            r"\b(shutdown|reboot|halt|poweroff|init\s+[06])\b",
            // Format/partition
            r"\b(format|fdisk|diskpart)\b",
            // Registry (Windows)
            r"reg\s+(add|delete|import)\s",
            r"schtasks\s+/(create|delete)",
            // netsh
            r"netsh\s",
        ])
        .expect("L0 regex patterns must compile")
    })
}

// ─── Syntax Gate ─────────────────────────────────────────────────

/// Check if script has syntax errors using simple heuristic analysis.
/// Returns SyntaxCheckResult with error details if syntax is invalid.
pub fn syntax_gate_check(code: &str, shell_type: &str) -> SyntaxCheckResult {
    let mut errors = Vec::new();

    // Check for NUL bytes (corrupted payload indicator)
    if code.contains('\0') {
        errors.push(SyntaxError {
            line: 0,
            column: 0,
            error_type: "nul_character".to_string(),
            message: "Script contains NUL character (possible corrupted payload)".to_string(),
            suggestion: "Remove NUL characters from the script".to_string(),
        });
    }

    // Check for unclosed quotes
    let mut in_single = false;
    let mut in_double = false;
    let mut line_num = 1;
    let mut col_num = 0;
    let mut single_quote_line = 0;
    let mut single_quote_col = 0;
    let mut double_quote_line = 0;
    let mut double_quote_col = 0;

    for ch in code.chars() {
        col_num += 1;
        if ch == '\n' {
            line_num += 1;
            col_num = 0;
            continue;
        }

        match ch {
            '\'' if !in_double => {
                if !in_single {
                    single_quote_line = line_num;
                    single_quote_col = col_num;
                }
                in_single = !in_single;
            }
            '"' if !in_single => {
                if !in_double {
                    double_quote_line = line_num;
                    double_quote_col = col_num;
                }
                in_double = !in_double;
            }
            _ => {}
        }
    }

    if in_single {
        errors.push(SyntaxError {
            line: single_quote_line,
            column: single_quote_col,
            error_type: "unclosed_quote".to_string(),
            message: "Unclosed single quote".to_string(),
            suggestion: format!(
                "Add closing single quote at line {} column {}",
                single_quote_line, single_quote_col
            ),
        });
    }

    if in_double {
        errors.push(SyntaxError {
            line: double_quote_line,
            column: double_quote_col,
            error_type: "unclosed_quote".to_string(),
            message: "Unclosed double quote".to_string(),
            suggestion: format!(
                "Add closing double quote at line {} column {}",
                double_quote_line, double_quote_col
            ),
        });
    }

    // Shell-specific syntax checks
    match shell_type {
        "bash" | "sh" => {
            // Check for unclosed if/for/while statements
            let if_count = code.matches("if ").count();
            let fi_count = code.matches("fi").count();
            if if_count > fi_count {
                errors.push(SyntaxError {
                    line: 0,
                    column: 0,
                    error_type: "unclosed_if".to_string(),
                    message: format!(
                        "Unclosed if statement: {} 'if' but only {} 'fi'",
                        if_count, fi_count
                    ),
                    suggestion: "Add missing 'fi' to close if statement".to_string(),
                });
            }

            // Check for unclosed for loops
            let for_count = code.matches("for ").count();
            let done_count = code.matches("done").count();
            if for_count > done_count {
                errors.push(SyntaxError {
                    line: 0,
                    column: 0,
                    error_type: "unclosed_for".to_string(),
                    message: format!(
                        "Unclosed for loop: {} 'for' but only {} 'done'",
                        for_count, done_count
                    ),
                    suggestion: "Add missing 'done' to close for loop".to_string(),
                });
            }
        }
        "powershell" | "pwsh" => {
            // Check for unclosed braces
            let open_braces = code.matches('{').count();
            let close_braces = code.matches('}').count();
            if open_braces != close_braces {
                errors.push(SyntaxError {
                    line: 0,
                    column: 0,
                    error_type: "unclosed_brace".to_string(),
                    message: format!(
                        "Mismatched braces: {} open but {} close",
                        open_braces, close_braces
                    ),
                    suggestion: "Balance opening and closing braces".to_string(),
                });
            }
        }
        _ => {}
    }

    SyntaxCheckResult {
        status: if errors.is_empty() {
            "passed".to_string()
        } else {
            "failed".to_string()
        },
        syntax_errors: errors,
    }
}

// ─── Risk Score Calculation ─────────────────────────────────────────────────

/// Calculate risk score (0-100) based on command analysis.
/// Implements patent formula: risk_score = Σ(wi × di) + Σ(pj)
pub fn calculate_risk_score(
    cmd_type: &str,
    code: &str,
) -> (u32, Vec<BlacklistHit>, Vec<PenaltyItem>, Vec<String>) {
    let lower = code.to_lowercase();
    let mut score: u32;
    let mut blacklist_hits = Vec::new();
    let mut penalty_items = Vec::new();
    let mut semantic_anomalies = Vec::new();

    // Dimension 1: Blacklist severity (weight 0.40)
    let mut blacklist_severity: f32 = 0.0;

    // Check L0 patterns
    if l0_regex_set().is_match(&lower) {
        blacklist_severity = 1.0;
        blacklist_hits.push(BlacklistHit {
            rule_id: "L0_PATTERN".to_string(),
            severity: "high".to_string(),
            matched: "System-critical pattern detected".to_string(),
            contribution: 40,
        });
    }

    // Check for destructive operations + system paths. Uses whole-word
    // matching so `rm -rf ~`, `rm\t-rf`, `/bin/rm -rf ~` are all caught while
    // `rmdir` / `alarm` are not.
    let has_destructive = destructive_command_regex().is_match(&lower) || lower.contains('>');

    // Special case: rm -rf / is always L0 (highest risk)
    let is_rm_rf_root = lower.contains("rm -rf /")
        || lower.contains("rm -rf /*")
        || lower.contains("rm -fr /")
        || lower.contains("rm -fr /*");

    if is_rm_rf_root {
        blacklist_severity = 1.0;
        blacklist_hits.push(BlacklistHit {
            rule_id: "RM_RF_ROOT".to_string(),
            severity: "critical".to_string(),
            matched: "rm -rf / detected - destroys entire filesystem".to_string(),
            contribution: 40,
        });
    } else if has_destructive && targets_system_path(&lower) {
        blacklist_severity = 1.0;
        blacklist_hits.push(BlacklistHit {
            rule_id: "SYSTEM_PATH_DESTRUCTIVE".to_string(),
            severity: "high".to_string(),
            matched: "Destructive operation on system path".to_string(),
            contribution: 40,
        });
    } else if has_destructive {
        // Destructive operation on user data (home dir, workspace, `*`, `.`,
        // `..`, redirection, ...). Previously this scored only ~25 (L4) and
        // auto-approved `rm -rf ~`, `rm -rf *`, `echo x > ~/.bashrc`, etc.
        blacklist_severity = 0.6;
        blacklist_hits.push(BlacklistHit {
            rule_id: "USER_DATA_DESTRUCTIVE".to_string(),
            severity: "medium".to_string(),
            matched: "Destructive operation on user data".to_string(),
            contribution: 24,
        });
    }

    // Sensitive reads (keys, credentials, shadow, .env, ...) must never be
    // auto-approved even when the executable is on the L6 whitelist. They
    // score high enough (>= 40 -> L3) to force user confirmation.
    if sensitive_read_detected(&lower) {
        if blacklist_severity < 1.0 {
            blacklist_severity = 1.0;
        }
        blacklist_hits.push(BlacklistHit {
            rule_id: "SENSITIVE_READ".to_string(),
            severity: "high".to_string(),
            matched: "Read of sensitive file or credential".to_string(),
            contribution: 40,
        });
    }

    // Penalty: wiping the home directory (`rm -rf ~`, `rm -rf $HOME`, ...).
    if home_wipe_regex().is_match(&lower) {
        penalty_items.push(PenaltyItem {
            name: "home_directory_wipe".to_string(),
            points: 30,
        });
        semantic_anomalies.push("home_directory_wipe".to_string());
    }

    // Penalty: wiping the current (workspace) directory (`rm -rf *`, `.`, `..`).
    if workspace_wipe_regex().is_match(&lower) {
        penalty_items.push(PenaltyItem {
            name: "workspace_wipe".to_string(),
            points: 25,
        });
        semantic_anomalies.push("workspace_wipe".to_string());
    }

    // Penalty: writing to a shell rc file (persistence / backdoor).
    if shell_rc_redirection_regex().is_match(&lower) {
        penalty_items.push(PenaltyItem {
            name: "shell_rc_redirection".to_string(),
            points: 25,
        });
        semantic_anomalies.push("shell_rc_redirection".to_string());
    }

    // Dimension 2: Whitelist coverage (weight 0.25)
    let whitelist_ratio = calculate_whitelist_coverage(cmd_type, code);
    let whitelist_score = 1.0 - whitelist_ratio;

    // Dimension 3: Semantic anomaly (weight 0.20)
    let mut anomaly_count = 0;

    // Check for download-and-execute pattern. Catches both `curl ... | bash`
    // and `curl -o /tmp/x ... && bash /tmp/x` (no pipe required).
    let has_download = lower.contains("curl")
        || lower.contains("wget")
        || lower.contains("invoke-webrequest")
        || lower.contains("iwr ")
        || lower.contains("start-bitstransfer");
    let has_exec = lower.contains("bash")
        || lower.contains("sh ")
        || lower.contains("powershell")
        || lower.contains("pwsh")
        || lower.contains("cmd /c");
    let chained =
        lower.contains('|') || lower.contains("&&") || lower.contains(';') || lower.contains('\n');
    if has_download && has_exec && chained {
        anomaly_count += 1;
        penalty_items.push(PenaltyItem {
            name: "download_and_execute".to_string(),
            points: 25,
        });
        semantic_anomalies.push("download_and_execute".to_string());
    }

    // Check for eval/exec/IEX
    if lower.contains("eval ")
        || lower.contains("exec ")
        || lower.contains("iex ")
        || lower.contains("invoke-expression")
    {
        anomaly_count += 1;
        penalty_items.push(PenaltyItem {
            name: "dynamic_execution".to_string(),
            points: 10,
        });
        semantic_anomalies.push("dynamic_exec".to_string());
    }

    // Check for base64 encoding
    if lower.contains("base64") || lower.contains("frombase64string") {
        anomaly_count += 1;
        penalty_items.push(PenaltyItem {
            name: "obfuscation".to_string(),
            points: 20,
        });
        semantic_anomalies.push("obfuscation".to_string());
    }

    // Check for high entropy (simplified)
    let entropy = calculate_entropy(&lower);
    if entropy > 6.0 {
        anomaly_count += 1;
        penalty_items.push(PenaltyItem {
            name: "high_entropy".to_string(),
            points: 20,
        });
        semantic_anomalies.push("high_entropy".to_string());
    }

    let semantic_anomaly_score = (anomaly_count as f32 / 5.0).min(1.0);

    // Dimension 4: Dangerous capability (weight 0.15)
    let mut dangerous_caps = 0;
    if lower.contains("/etc/") || lower.contains("/root/") || lower.contains("c:\\windows") {
        dangerous_caps += 1;
        penalty_items.push(PenaltyItem {
            name: "write_system_dir".to_string(),
            points: 15,
        });
    }
    if lower.contains("curl") || lower.contains("wget") || lower.contains("invoke-webrequest") {
        dangerous_caps += 1;
    }
    if lower.contains("chmod") || lower.contains("chown") || lower.contains("icacls") {
        dangerous_caps += 1;
    }
    if lower.contains("dd ") || lower.contains("mkfs") || lower.contains("format") {
        dangerous_caps += 1;
    }
    // Destructive commands (rm/mv/del/...) and redirection are dangerous even
    // when they do not target a system path — previously `rm -rf ~` scored 0
    // here and auto-approved at L4.
    if destructive_command_regex().is_match(&lower) {
        dangerous_caps += 1;
    }
    if lower.contains('>') || lower.contains(">>") {
        dangerous_caps += 1;
    }
    // Sensitive reads (keys, credentials, shadow, .env, ...).
    if sensitive_read_detected(&lower) {
        dangerous_caps += 1;
    }

    let dangerous_capability_score = (dangerous_caps as f32 / 5.0).min(1.0);

    // Calculate weighted score
    let weighted_score = (blacklist_severity * 0.40
        + whitelist_score * 0.25
        + semantic_anomaly_score * 0.20
        + dangerous_capability_score * 0.15)
        * 100.0;

    // Add penalty points
    let penalty_points: u32 = penalty_items.iter().map(|p| p.points).sum();

    score = (weighted_score as u32)
        .saturating_add(penalty_points)
        .min(100);

    // Add high-risk command penalty (rm -rf / gets +30 penalty)
    if is_rm_rf_root {
        penalty_items.push(PenaltyItem {
            name: "high_risk_command".to_string(),
            points: 30,
        });
        score = score.saturating_add(30).min(100);
    }

    (score, blacklist_hits, penalty_items, semantic_anomalies)
}

/// Calculate whitelist coverage ratio for a command.
fn calculate_whitelist_coverage(cmd_type: &str, code: &str) -> f32 {
    if cmd_type == "direct" {
        let parts = split_command_line(code);
        if let Some(first) = parts.first() {
            let exe_name = Path::new(first.as_str())
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(first.as_str())
                .to_lowercase();

            if sorted_contains(L6_EXECUTABLES, &exe_name) {
                return 1.0; // Fully covered by whitelist
            }
        }
        return 0.0;
    }

    // For shell scripts, check if all commands are in whitelist. Split on
    // newlines too — previously a multi-line script such as
    // `cat /etc/passwd\nrm -rf ~` was treated as ONE command starting with
    // `cat`, giving 100% whitelist coverage and an L6 auto-approval.
    let commands: Vec<&str> = code
        .split(&['|', ';', '&', '\n'][..])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if commands.is_empty() {
        return 1.0;
    }

    let mut covered = 0;
    for cmd in &commands {
        let first_word = cmd.split_whitespace().next().unwrap_or("");
        let exe_name = Path::new(first_word)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(first_word)
            .to_lowercase();

        if sorted_contains(L6_EXECUTABLES, &exe_name) {
            covered += 1;
        }
    }

    covered as f32 / commands.len() as f32
}

/// Calculate Shannon entropy of a string.
fn calculate_entropy(s: &str) -> f32 {
    if s.is_empty() {
        return 0.0;
    }

    let mut char_counts = std::collections::HashMap::new();
    for ch in s.chars() {
        *char_counts.entry(ch).or_insert(0) += 1;
    }

    let len = s.len() as f32;
    let mut entropy = 0.0;

    for count in char_counts.values() {
        let p = *count as f32 / len;
        entropy -= p * p.log2();
    }

    entropy
}

/// Generate structured risk assessment report.
pub fn generate_risk_report(cmd_type: &str, code: &str, request_id: &str) -> RiskAssessmentReport {
    // Step 1: Syntax gate check
    let syntax_check = syntax_gate_check(code, cmd_type);

    if syntax_check.status == "failed" {
        return RiskAssessmentReport {
            request_id: request_id.to_string(),
            risk_level: "REJECTED".to_string(),
            risk_score: 0,
            disposition: "Invalid syntax, refuse to rate".to_string(),
            dimension_scores: DimensionScores {
                blacklist_severity: 0.0,
                whitelist_coverage: 0.0,
                semantic_anomaly: 0.0,
                dangerous_capability: 0.0,
            },
            blacklist_hits: vec![],
            penalty_items: vec![],
            whitelist_coverage: WhitelistCoverage {
                covered: 0,
                total: 0,
                ratio: 0.0,
            },
            semantic_anomalies: vec![],
            recommendation: "Fix syntax errors and resubmit".to_string(),
            syntax_check,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
    }

    // Step 2: Calculate risk score
    let (score, blacklist_hits, penalty_items, semantic_anomalies) =
        calculate_risk_score(cmd_type, code);

    // Step 3: Map to risk level
    let risk_level = score_to_risk_level(score);

    // Step 4: Generate disposition
    let disposition = if score >= REJECT_THRESHOLD {
        "Force reject suggestion (score exceeds rejection threshold)".to_string()
    } else {
        risk_level.disposition().to_string()
    };

    // Step 5: Generate recommendation
    let recommendation = if score >= REJECT_THRESHOLD {
        format!(
            "Reject execution: score {} ≥ reject threshold {}",
            score, REJECT_THRESHOLD
        )
    } else if risk_level.requires_human_review() {
        format!(
            "Route to manual approval flow: {}",
            risk_level.description()
        )
    } else if risk_level.requires_llm_confirmation() {
        format!(
            "LLM warning and confirm execution: {}",
            risk_level.description()
        )
    } else {
        format!("Recommend to allow: {}", risk_level.description())
    };

    let whitelist_ratio = calculate_whitelist_coverage(cmd_type, code);
    let commands: Vec<&str> = code
        .split(&['|', ';', '&'][..])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    RiskAssessmentReport {
        request_id: request_id.to_string(),
        risk_level: format!("{:?}", risk_level),
        risk_score: score,
        disposition,
        dimension_scores: DimensionScores {
            blacklist_severity: if blacklist_hits.is_empty() { 0.0 } else { 1.0 },
            whitelist_coverage: whitelist_ratio,
            semantic_anomaly: (semantic_anomalies.len() as f32 / 5.0).min(1.0),
            dangerous_capability: 0.0, // Calculated in calculate_risk_score
        },
        blacklist_hits,
        penalty_items,
        whitelist_coverage: WhitelistCoverage {
            covered: (whitelist_ratio * commands.len() as f32) as usize,
            total: commands.len(),
            ratio: whitelist_ratio,
        },
        semantic_anomalies,
        recommendation,
        syntax_check,
        timestamp: chrono::Utc::now().to_rfc3339(),
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn sorted_contains(slice: &[&str], needle: &str) -> bool {
    slice.binary_search(&needle).is_ok()
}

fn targets_system_path(args_lower: &str) -> bool {
    SYSTEM_PATH_PREFIXES
        .iter()
        .any(|prefix| args_lower.contains(prefix))
}

// ─── Destructive / sensitive detection helpers ───────────────────────────────

/// Destructive commands matched as whole words (handles tabs, multiple spaces,
/// and paths like `/bin/rm`). `\b` prevents matching `rmdir`, `alarm`, etc.
fn destructive_command_regex() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"\b(?:rm|del|erase|mv|move|chmod|chown|chgrp|dd|rmdir|shred|truncate|unlink|mkfs|format|fdisk|diskpart|mount|umount|tee)\b",
        )
        .expect("destructive command regex must compile")
    })
}

/// `rm` (or similar) targeting the home directory / `$HOME` / `/home` / `/Users`.
fn home_wipe_regex() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"\b(?:rm|del|erase|shred|truncate|unlink|rmdir)\b[^\n;|&]*?(?:~|~/?|\$home\b|/home/|/users/)")
            .expect("home wipe regex must compile")
    })
}

/// `rm -rf *` / `rm -rf .` / `rm -rf ..` — wipes the current (workspace) directory.
/// Uses `\.\.?/` / `\.\.?$` so `rm -rf .git` is NOT a false positive.
fn workspace_wipe_regex() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"\brm\b[^\n;|&]*?\b(?:-rf|-fr)\b[^\n;|&]*?(?:\*|\.\.?/|\.\.?$)")
            .expect("workspace wipe regex must compile")
    })
}

/// Redirection (`>` / `>>`) into a shell rc file (`.bashrc`, `.zshrc`, ...) —
/// a classic persistence / backdoor vector.
fn shell_rc_redirection_regex() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r">>?\s*(?:~|/home/|/users/)?[^;\n|&]*(?:\.bashrc|\.zshrc|\.profile|\.bash_profile|\.bash_login|\.zprofile|\.bash_aliases)",
        )
        .expect("shell rc redirection regex must compile")
    })
}

/// Reads of credentials / keys / sensitive configs that should never run
/// without explicit user confirmation.
fn sensitive_read_detected(lower: &str) -> bool {
    const SENSITIVE: &[&str] = &[
        "~/.ssh",
        "/etc/shadow",
        "/etc/passwd",
        "/etc/sudoers",
        "/etc/sudoers.d",
        "id_rsa",
        "id_ed25519",
        "id_dsa",
        "id_ecdsa",
        ".env",
        ".env.local",
        ".env.production",
        "aws/credentials",
        "~/.aws",
        "~/.gnupg",
        ".git-credentials",
        ".netrc",
        "hkey_local_machine",
        "hklm",
        "system32/config",
        "~/.bash_history",
        "~/.zsh_history",
        "~/.kube",
        "kubeconfig",
        "~/.docker/config.json",
        "~/.npmrc",
        "~/.pypirc",
        "~/.m2/settings.xml",
    ];
    SENSITIVE.iter().any(|s| lower.contains(s))
}
