// ─── Unified patch application (LLM-hardened) ──────────────────────────────
//
// `diffy::apply` is a spec-accurate applier, and LLM-written patches are
// almost never spec-accurate, so most real patches used to be rejected before
// any of them ran:
//
//   * `@@` headers must literally read `@@ -a,b +c,d @@`, the declared counts
//     must equal the number of body lines exactly, and hunks must be sorted
//     by those declared numbers without overlapping — three things models get
//     wrong constantly, and one mismatch aborts the whole patch;
//   * every context line must match the file byte-for-byte — one changed
//     indent or trailing space fails the hunk with no hint of where or why;
//   * there is no fuzz, no whitespace tolerance and no retry advice.
//
// The parser + applier below instead treat the `@@` numbers as a position
// hint and match hunks by content:
//
//   1. parse leniently — headers optional, counts ignored, body lines that
//      lost their leading space treated as context, blank separators, code
//      fences and trailing prose tolerated;
//   2. locate every hunk by searching for its context + removed lines using
//      exact → trailing-whitespace-insensitive → whitespace-insensitive
//      comparison, preferring the position closest to the `@@` hint;
//   3. apply GNU patch's default fuzz of two edge context lines;
//   4. keep the file's own bytes for unchanged context lines, so a lenient
//      match never rewrites the whitespace of untouched lines;
//   5. report which hunk failed, the closest actual content and how to fix
//      the patch — plus notes whenever leniency was needed.

/// GNU patch applies with a default fuzz factor of 2: up to two edge context
/// lines may differ from the file and the hunk is still accepted.
const PATCH_MAX_FUZZ: usize = 2;

/// Cap on how many lines each diagnostic block prints.
const PATCH_DIAGNOSTIC_LINES: usize = 14;

/// How a hunk line participates in the diff: context lines belong to both the
/// old and the new side, add/delete lines to only one of them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PatchLineKind {
    Context,
    Add,
    Delete,
}

/// One `@@` hunk, parsed leniently. Line numbers and counts from the header
/// are kept only as a position hint — matching uses the line content.
#[derive(Default)]
struct ParsedPatchHunk {
    /// 1-based first old-side line declared by the `@@` header, when present.
    old_start: Option<usize>,
    /// Declared old-side line count (`Some(0)` marks an insertion point).
    old_count: Option<usize>,
    old_lines: Vec<String>,
    old_kinds: Vec<PatchLineKind>,
    new_lines: Vec<String>,
    new_kinds: Vec<PatchLineKind>,
    /// The last new-side line is followed by `\ No newline at end of file`.
    new_no_final_newline: bool,
}

impl ParsedPatchHunk {
    fn is_empty(&self) -> bool {
        self.old_lines.is_empty() && self.new_lines.is_empty()
    }

    fn has_changes(&self) -> bool {
        self.old_kinds
            .iter()
            .any(|kind| *kind == PatchLineKind::Delete)
            || self
                .new_kinds
                .iter()
                .any(|kind| *kind == PatchLineKind::Add)
    }

    fn push_context(&mut self, text: &str) {
        self.old_lines.push(text.to_string());
        self.old_kinds.push(PatchLineKind::Context);
        self.new_lines.push(text.to_string());
        self.new_kinds.push(PatchLineKind::Context);
        self.new_no_final_newline = false;
    }

    fn push_addition(&mut self, text: &str) {
        self.new_lines.push(text.to_string());
        self.new_kinds.push(PatchLineKind::Add);
        self.new_no_final_newline = false;
    }

    fn push_deletion(&mut self, text: &str) {
        self.old_lines.push(text.to_string());
        self.old_kinds.push(PatchLineKind::Delete);
    }
}

/// Parse `@@ -12,3 +12,5 @@`. The counts are optional (`@@ -12 +12 @@`) and
/// are never validated against the body, only used as a position hint.
fn parse_hunk_header(header: &str) -> Option<(usize, usize)> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"^@@+\s*-(\d+)(?:,(\d+))?\s+\+\d+")
            .expect("hunk header regex must compile")
    });
    let captures = re.captures(header)?;
    let start = captures.get(1)?.as_str().parse().ok()?;
    let count = captures
        .get(2)
        .and_then(|value| value.as_str().parse().ok())
        .unwrap_or(1);
    Some((start, count))
}

fn flush_pending_lenient_context(hunk: &mut ParsedPatchHunk, pending: &mut Vec<String>) {
    for line in pending.drain(..) {
        hunk.push_context(&line);
    }
}

/// Lenient unified-diff parser.
///
/// Accepted without complaint: missing `---`/`+++` headers, `@@` headers with
/// wrong or missing counts, hunks in any order, blank separators between
/// hunks, ```diff fences, body lines that lost their leading space, trailing
/// prose after the last hunk, and `\ No newline at end of file` markers.
fn parse_unified_patch(patch: &str) -> Result<Vec<ParsedPatchHunk>, String> {
    let mut hunks: Vec<ParsedPatchHunk> = Vec::new();
    let mut current: Option<ParsedPatchHunk> = None;
    // Blank lines and unprefixed lines are held back: inside a hunk they are
    // empty/lossy context lines, but a separator before the next `@@` or
    // trailing prose after the last hunk is not content. They are only flushed
    // into the hunk once another prefixed body line follows.
    let mut pending_lenient: Vec<String> = Vec::new();
    let mut saw_file_header = false;
    let mut last_kind: Option<PatchLineKind> = None;

    for raw in patch.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);

        // Models sometimes wrap the diff in ```diff fences.
        if line.starts_with("```") {
            continue;
        }

        if line.starts_with("@@") {
            if let Some(previous) = current.take() {
                if !previous.is_empty() {
                    hunks.push(previous);
                }
            }
            pending_lenient.clear();
            let mut hunk = ParsedPatchHunk::default();
            if let Some((start, count)) = parse_hunk_header(line) {
                hunk.old_start = Some(start);
                hunk.old_count = Some(count);
            }
            current = Some(hunk);
            last_kind = None;
            continue;
        }

        if current.is_none() {
            if line.starts_with("+++") {
                saw_file_header = true;
                continue;
            }
            if line.starts_with("---")
                || line.starts_with("diff ")
                || line.starts_with("index ")
                || line.starts_with("Index: ")
                || line.starts_with("new file mode")
                || line.starts_with("deleted file mode")
                || line.starts_with("=== ")
                || line.starts_with("*** ")
            {
                continue;
            }
            // An implicit hunk: prefixed body lines directly after a `+++`
            // header, with no `@@` line at all. Other preamble noise (mail
            // headers, commit messages) is skipped instead.
            if saw_file_header && !line.is_empty() && !line.starts_with('\\') {
                current = Some(ParsedPatchHunk::default());
            } else {
                continue;
            }
        }

        let hunk = current.as_mut().expect("hunk was just initialized");

        if line.is_empty() {
            pending_lenient.push(String::new());
            continue;
        }

        let mut chars = line.chars();
        let prefix = chars.next().expect("line is not empty");
        let text = chars.as_str();

        match prefix {
            ' ' => {
                flush_pending_lenient_context(hunk, &mut pending_lenient);
                hunk.push_context(text);
                last_kind = Some(PatchLineKind::Context);
            }
            '+' => {
                flush_pending_lenient_context(hunk, &mut pending_lenient);
                hunk.push_addition(text);
                last_kind = Some(PatchLineKind::Add);
            }
            '-' => {
                flush_pending_lenient_context(hunk, &mut pending_lenient);
                hunk.push_deletion(text);
                last_kind = Some(PatchLineKind::Delete);
            }
            '\\' => {
                // `\ No newline at end of file` — refers to the line above.
                if matches!(
                    last_kind,
                    Some(PatchLineKind::Add) | Some(PatchLineKind::Context)
                ) {
                    hunk.new_no_final_newline = true;
                }
            }
            _ => {
                // Leniency: a body line that lost its leading space is still a
                // context line. Deferred like blank lines so trailing prose
                // after the last hunk never becomes accidental context.
                pending_lenient.push(line.to_string());
            }
        }
    }

    if let Some(previous) = current.take() {
        if !previous.is_empty() {
            hunks.push(previous);
        }
    }

    if hunks.is_empty() {
        return Err(concat!(
            "the patch contains no `@@` hunk — it does not look like a unified diff.\n",
            "Expected form (LF or CRLF; `---`/`+++` headers and counts optional):\n",
            "\n",
            "@@ -12,3 +12,3 @@\n",
            " a context line copied from the file\n",
            "-the old line\n",
            "+the new line\n",
            " another context line copied from the file\n"
        )
        .to_string());
    }

    Ok(hunks)
}

/// Strip a uniform leading indentation from every line of the patch, so the
/// `@@`/`+`/`-`/` ` prefixes sit in column 0 even when the model indented the
/// whole diff.
fn dedent_patch(patch: &str) -> String {
    let indent = patch
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            line.len()
                - line
                    .trim_start_matches(|ch: char| ch == ' ' || ch == '\t')
                    .len()
        })
        .min()
        .unwrap_or(0);
    if indent == 0 {
        return patch.to_string();
    }

    let mut dedented = String::with_capacity(patch.len());
    for (index, line) in patch.lines().enumerate() {
        if index > 0 {
            dedented.push('\n');
        }
        if line.len() >= indent
            && line.as_bytes()[..indent]
                .iter()
                .all(|byte| *byte == b' ' || *byte == b'\t')
        {
            dedented.push_str(&line[indent..]);
        } else {
            dedented.push_str(line);
        }
    }
    dedented
}

/// How leniently a hunk's old side may match the file, in increasing order of
/// tolerance. The strictest level is tried first so an exact location wins.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PatchMatchTolerance {
    Exact,
    TrailingWhitespace,
    AnyWhitespace,
}

impl PatchMatchTolerance {
    fn all() -> [PatchMatchTolerance; 3] {
        [
            PatchMatchTolerance::Exact,
            PatchMatchTolerance::TrailingWhitespace,
            PatchMatchTolerance::AnyWhitespace,
        ]
    }

    fn matches(self, actual: &str, expected: &str) -> bool {
        match self {
            PatchMatchTolerance::Exact => actual == expected,
            PatchMatchTolerance::TrailingWhitespace => actual.trim_end() == expected.trim_end(),
            PatchMatchTolerance::AnyWhitespace => actual.trim() == expected.trim(),
        }
    }

    fn describe(self) -> &'static str {
        match self {
            PatchMatchTolerance::Exact => "an exact match",
            PatchMatchTolerance::TrailingWhitespace => "trailing-whitespace-insensitive matching",
            PatchMatchTolerance::AnyWhitespace => {
                "whitespace-insensitive matching (indentation differs)"
            }
        }
    }
}

fn find_hunk_core_matches(
    base: &[&str],
    core: &[String],
    tolerance: PatchMatchTolerance,
) -> Vec<usize> {
    if core.is_empty() || base.len() < core.len() {
        return Vec::new();
    }
    (0..=base.len() - core.len())
        .filter(|&start| {
            core.iter()
                .enumerate()
                .all(|(offset, expected)| tolerance.matches(base[start + offset], expected))
        })
        .collect()
}

fn pick_nearest_match(hits: &[usize], expected: Option<usize>) -> usize {
    match expected {
        Some(expected) => *hits
            .iter()
            .min_by_key(|hit| hit.abs_diff(expected))
            .expect("hits is never empty here"),
        None => hits[0],
    }
}

/// One hunk matched against the file, ready to splice.
struct LocatedHunkEdit {
    /// Index of the hunk in the patch (0-based, for messages).
    order: usize,
    /// 0-based line index in the original file where the replacement starts.
    start: usize,
    /// Number of original lines the replacement replaces.
    replaced_len: usize,
    /// Replacement lines (added lines from the patch, context from the file).
    replacement: Vec<String>,
    /// Edge context lines ignored by fuzzing.
    lead_drop: usize,
    trail_drop: usize,
    tolerance: PatchMatchTolerance,
    /// Raw 1-based `@@` start, when the header had one.
    header_line: Option<usize>,
    /// How many places the core matched (the closest to the hint was used).
    candidate_count: usize,
    /// Whether the hunk actually adds or removes lines.
    had_changes: bool,
}

/// Build the replacement text for a matched core window. Context lines reuse
/// the file's actual bytes (so a whitespace-lenient match never rewrites lines
/// that are not part of the change); added lines come from the patch.
fn build_hunk_replacement(
    base: &[&str],
    start: usize,
    hunk: &ParsedPatchHunk,
    lead_drop: usize,
    trail_drop: usize,
) -> Vec<String> {
    let old_end = hunk.old_lines.len().saturating_sub(trail_drop);
    let new_end = hunk.new_lines.len().saturating_sub(trail_drop);
    let mut replacement = Vec::new();
    let mut old_index = lead_drop;
    let mut new_index = lead_drop;

    while new_index < new_end {
        // Old-side deletions have no counterpart on the new side.
        while old_index < old_end && hunk.old_kinds[old_index] == PatchLineKind::Delete {
            old_index += 1;
        }
        match hunk.new_kinds[new_index] {
            PatchLineKind::Add => {
                replacement.push(hunk.new_lines[new_index].clone());
                new_index += 1;
            }
            PatchLineKind::Context => {
                let file_line = base
                    .get(start + old_index.saturating_sub(lead_drop))
                    .copied()
                    .or_else(|| hunk.old_lines.get(old_index).map(String::as_str))
                    .unwrap_or("");
                replacement.push(file_line.to_string());
                old_index += 1;
                new_index += 1;
            }
            PatchLineKind::Delete => {
                // A deletion never appears on the new side; keep the walk in
                // step defensively.
                old_index += 1;
                new_index += 1;
            }
        }
    }

    replacement
}

/// Locate one hunk in the file by content. Preference order: less fuzz before
/// more, stricter match before looser, and for equal quality the position
/// closest to the `@@` hint.
fn locate_hunk_edit(
    base: &[&str],
    hunk: &ParsedPatchHunk,
    order: usize,
    hunk_total: usize,
) -> Result<LocatedHunkEdit, String> {
    let expected = hunk.old_start.map(|start| {
        if hunk.old_count == Some(0) {
            // `-N,0` marks an insertion point right after line N.
            start
        } else {
            start.saturating_sub(1)
        }
    });

    if hunk.old_lines.is_empty() {
        // Pure insertion (`@@ -12,0 +13,3 @@`): there is no content to search
        // for, so the header position is the only anchor available.
        let Some(hint) = expected else {
            return Err(format!(
                "hunk {}/{} only inserts lines but has no usable `@@` position and no context — nothing was written.\n\
                 Add a `@@ -<line>,0 +<line>,<count> @@` header (0 = before the first line) or 3 context lines.",
                order + 1,
                hunk_total
            ));
        };
        let start = hint.min(base.len());
        return Ok(LocatedHunkEdit {
            order,
            start,
            replaced_len: 0,
            replacement: hunk.new_lines.clone(),
            lead_drop: 0,
            trail_drop: 0,
            tolerance: PatchMatchTolerance::Exact,
            header_line: hunk.old_start,
            candidate_count: 1,
            had_changes: hunk.has_changes(),
        });
    }

    let lead_context = hunk
        .old_kinds
        .iter()
        .take_while(|kind| **kind == PatchLineKind::Context)
        .count();
    let trail_context = hunk
        .old_kinds
        .iter()
        .rev()
        .take_while(|kind| **kind == PatchLineKind::Context)
        .count();
    // Fuzzing may only drop context lines, never removed lines.
    let max_lead_drop = lead_context.min(PATCH_MAX_FUZZ);
    let max_trail_drop = trail_context.min(PATCH_MAX_FUZZ);

    let mut ambiguous: Option<Vec<usize>> = None;

    for total_drop in 0..=PATCH_MAX_FUZZ {
        for lead_drop in 0..=total_drop.min(max_lead_drop) {
            let trail_drop = total_drop - lead_drop;
            if trail_drop > max_trail_drop
                || lead_drop + trail_drop >= hunk.old_lines.len()
                || lead_drop + trail_drop > hunk.new_lines.len()
            {
                continue;
            }
            let core_old = &hunk.old_lines[lead_drop..hunk.old_lines.len() - trail_drop];
            if core_old.is_empty() {
                continue;
            }
            for tolerance in PatchMatchTolerance::all() {
                let hits = find_hunk_core_matches(base, core_old, tolerance);
                if hits.is_empty() {
                    continue;
                }
                if hits.len() > 1 && expected.is_none() {
                    ambiguous.get_or_insert_with(|| hits.clone());
                    continue;
                }
                let core_expected = expected.map(|hint| hint + lead_drop);
                let start = pick_nearest_match(&hits, core_expected);
                return Ok(LocatedHunkEdit {
                    order,
                    start,
                    replaced_len: core_old.len(),
                    replacement: build_hunk_replacement(base, start, hunk, lead_drop, trail_drop),
                    lead_drop,
                    trail_drop,
                    tolerance,
                    header_line: hunk.old_start,
                    candidate_count: hits.len(),
                    had_changes: hunk.has_changes(),
                });
            }
        }
    }

    if let Some(hits) = ambiguous {
        let places = hits
            .iter()
            .take(5)
            .map(|hit| (hit + 1).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let more = if hits.len() > 5 { ", ..." } else { "" };
        let mut message = format!(
            "hunk {}/{} matches {} different places in the file (starting at line(s) {}{}) and its `@@` header has no usable line number — nothing was written.\n",
            order + 1,
            hunk_total,
            hits.len(),
            places,
            more
        );
        message.push_str(concat!(
            "\nFix one of these and retry:\n",
            "  - add `@@ -<line>,<count> +<line>,<count> @@` numbers (copy them from a fresh `read`); or\n",
            "  - include more unchanged context lines so the hunk can only match one place.\n"
        ));
        return Err(message);
    }

    Err(describe_hunk_failure(base, hunk, order, hunk_total))
}

/// Rich failure report: what the hunk expects, what the file actually
/// contains near the most likely location, and how to repair the patch.
fn describe_hunk_failure(
    base: &[&str],
    hunk: &ParsedPatchHunk,
    order: usize,
    hunk_total: usize,
) -> String {
    let mut message = format!(
        "hunk {}/{} does not match the current file content — nothing was written.\n",
        order + 1,
        hunk_total
    );

    message.push_str("\nWhat the patch expects:\n");
    for (index, line) in hunk.old_lines.iter().enumerate() {
        if index >= PATCH_DIAGNOSTIC_LINES {
            message.push_str("  ...\n");
            break;
        }
        let marker = if hunk.old_kinds[index] == PatchLineKind::Delete {
            '-'
        } else {
            ' '
        };
        message.push_str(&format!("{}{}\n", marker, line));
    }

    if hunk.old_lines.is_empty() {
        message.push_str(concat!(
            "\nThe hunk has no context or removed lines, so there is nothing to search for.\n",
            "Add a `@@ -<line>,0 +<line>,<count> @@` header, or 3 unchanged context lines.\n"
        ));
        return message;
    }

    // Show what the file actually contains where the hunk most likely belongs:
    // the window with the most matching lines.
    if base.len() >= hunk.old_lines.len() && hunk.old_lines.len() <= 512 {
        let mut best: Option<(usize, usize)> = None;
        for start in 0..=base.len() - hunk.old_lines.len() {
            let matched = hunk
                .old_lines
                .iter()
                .enumerate()
                .filter(|(offset, expected)| {
                    PatchMatchTolerance::AnyWhitespace.matches(base[start + offset], expected)
                })
                .count();
            if best.map_or(true, |(_, best_matched)| matched > best_matched) {
                best = Some((start, matched));
            }
        }
        if let Some((start, matched)) = best {
            message.push_str(&format!(
                "\nClosest current content (line {}, {} of {} hunk lines match):\n",
                start + 1,
                matched,
                hunk.old_lines.len()
            ));
            let end = (start + hunk.old_lines.len()).min(base.len());
            for (index, line) in base[start..end].iter().enumerate() {
                if index >= PATCH_DIAGNOSTIC_LINES {
                    message.push_str("  ...\n");
                    break;
                }
                message.push_str(&format!(" {}\n", line));
            }
        }
    }

    // A common retry loop: the change was already applied, so the patch's
    // *result* is what now sits in the file.
    if !hunk.new_lines.is_empty()
        && !find_hunk_core_matches(base, &hunk.new_lines, PatchMatchTolerance::Exact).is_empty()
    {
        message.push_str(concat!(
            "\nNote: the patch's result content already exists in the file — this change may already be applied.\n",
            "Re-read the file and drop the hunk if it is no longer needed.\n"
        ));
    }

    message.push_str(concat!(
        "\nHow to fix it:\n",
        "  - call file_actions {\"action\":\"read\"} and copy every context and removed line character-for-character\n",
        "    (same indentation, context lines keep one leading space);\n",
        "  - include 3 unchanged context lines directly above and below the change;\n",
        "  - the `@@` line numbers are only a hint — matching is done on content, so fix the content, not the numbers;\n",
        "  - if the block was rewritten rather than edited, use action \"write\" with the complete new file content instead.\n"
    ));

    message
}

/// Apply every parsed hunk: locate each one in the original file, then splice
/// all edits in ascending position order.
fn apply_parsed_patch(
    original: &str,
    hunks: &[ParsedPatchHunk],
) -> Result<(String, Vec<String>), String> {
    let base: Vec<&str> = original.split('\n').collect();

    let mut edits: Vec<LocatedHunkEdit> = Vec::with_capacity(hunks.len());
    for (order, hunk) in hunks.iter().enumerate() {
        edits.push(locate_hunk_edit(&base, hunk, order, hunks.len())?);
    }

    edits.sort_by_key(|edit| (edit.start, edit.order));
    for pair in edits.windows(2) {
        let (previous, next) = (&pair[0], &pair[1]);
        if next.start < previous.start + previous.replaced_len {
            return Err(format!(
                "hunks {} and {} overlap in the file (around line {}) — merge them into one hunk and retry.",
                previous.order + 1,
                next.order + 1,
                next.start + 1
            ));
        }
    }

    let mut lines: Vec<String> = Vec::with_capacity(base.len());
    let mut cursor = 0usize;
    for edit in &edits {
        lines.extend(
            base[cursor..edit.start]
                .iter()
                .map(|line| (*line).to_string()),
        );
        lines.extend(edit.replacement.iter().cloned());
        cursor = edit.start + edit.replaced_len;
    }
    lines.extend(base[cursor..].iter().map(|line| (*line).to_string()));

    let mut patched = lines.join("\n");

    // Trailing-newline bookkeeping, driven by the last hunk's new side.
    if let Some(last) = hunks.last() {
        if last.new_no_final_newline {
            if patched.ends_with('\n') {
                patched.pop();
            }
        } else if last.new_kinds.last() == Some(&PatchLineKind::Add) && !patched.ends_with('\n') {
            patched.push('\n');
        }
    }

    Ok((patched, build_patch_notes(&edits)))
}

/// Notes describing every leniency the applier had to use. They travel back to
/// the model in the tool result so the next patch can be written tighter.
fn build_patch_notes(edits: &[LocatedHunkEdit]) -> Vec<String> {
    let mut notes = Vec::new();
    for edit in edits {
        if notes.len() >= 6 {
            break;
        }
        if !edit.had_changes {
            notes.push(format!(
                "hunk {}: context only, no added or removed lines — nothing changed",
                edit.order + 1
            ));
        }
        if let Some(header_line) = edit.header_line {
            if edit.start.abs_diff(header_line.saturating_sub(1)) >= 3 {
                notes.push(format!(
                    "hunk {}: located at line {} by content search (the `@@` header said line {})",
                    edit.order + 1,
                    edit.start + 1,
                    header_line
                ));
            }
        }
        if edit.candidate_count > 1 {
            notes.push(format!(
                "hunk {}: {} places matched; the one closest to line {} was used",
                edit.order + 1,
                edit.candidate_count,
                edit.start + 1
            ));
        }
        if edit.lead_drop > 0 || edit.trail_drop > 0 {
            notes.push(format!(
                "hunk {}: applied with fuzz ({} leading / {} trailing context line(s) ignored)",
                edit.order + 1,
                edit.lead_drop,
                edit.trail_drop
            ));
        }
        if edit.tolerance != PatchMatchTolerance::Exact {
            notes.push(format!(
                "hunk {}: matched with {}",
                edit.order + 1,
                edit.tolerance.describe()
            ));
        }
    }
    notes
}

/// Apply a unified diff to file content.
///
/// Returns the patched text plus notes describing every leniency the applier
/// had to use (content-search drift, whitespace tolerance, fuzz, ...).
pub(super) fn apply_unified_patch(
    original: &str,
    patch_str: &str,
) -> Result<(String, Vec<String>), String> {
    let uses_crlf = original.contains("\r\n");
    let normalized_original = original.replace("\r\n", "\n");
    let normalized_patch = patch_str
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");

    let hunks = match parse_unified_patch(&normalized_patch) {
        Ok(hunks) => hunks,
        Err(err) => {
            // Retry once with the common leading indentation stripped: models
            // sometimes indent the whole diff when quoting it.
            let dedented = dedent_patch(&normalized_patch);
            if dedented == normalized_patch {
                return Err(err);
            }
            parse_unified_patch(&dedented)?
        }
    };

    let (mut patched, notes) = apply_parsed_patch(&normalized_original, &hunks)?;

    // Restore the original line-ending style.
    if uses_crlf {
        patched = patched.replace('\n', "\r\n");
    }

    Ok((patched, notes))
}
