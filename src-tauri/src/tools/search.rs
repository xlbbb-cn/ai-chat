// Integrated search engine for `file_actions`, `skill_read` and the memory
// store: glob/space matcher, budgeted parallel tree walk and output renderer.

use ignore::WalkState;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

enum SearchMatcher {
    Literal {
        needle: String,
        needle_lower: String,
        case_sensitive: bool,
    },
    Regex(regex::Regex),
}

impl SearchMatcher {
    fn build(query: &str, case_sensitive: bool, use_regex: bool) -> Result<Self, String> {
        if use_regex {
            let regex = regex::RegexBuilder::new(query)
                .case_insensitive(!case_sensitive)
                .build()
                .map_err(|e| format!("Invalid search regex: {}", e))?;
            Ok(Self::Regex(regex))
        } else {
            Ok(Self::Literal {
                needle: query.to_string(),
                needle_lower: query.to_lowercase(),
                case_sensitive,
            })
        }
    }

    fn is_match(&self, line: &str) -> bool {
        match self {
            SearchMatcher::Literal {
                needle,
                needle_lower,
                case_sensitive,
            } => {
                if *case_sensitive {
                    line.contains(needle)
                } else {
                    contains_ignore_case(line, needle_lower)
                }
            }
            SearchMatcher::Regex(regex) => regex.is_match(line),
        }
    }
}

/// Case-insensitive `contains` that does not allocate. The previous version
/// lowercased *every line of every file*, i.e. one `String` allocation per
/// line — hundreds of thousands of them on a large tree. ASCII folding is
/// byte-identical to `str::to_lowercase` for ASCII input, so it is only used
/// when both sides are ASCII; anything else falls back to the allocating path
/// so Unicode casing behaviour is unchanged.
pub(super) fn contains_ignore_case(haystack: &str, needle_lower: &str) -> bool {
    if !(haystack.is_ascii() && needle_lower.is_ascii()) {
        return haystack.to_lowercase().contains(needle_lower);
    }

    let hay = haystack.as_bytes();
    let needle = needle_lower.as_bytes();
    if needle.is_empty() {
        return true;
    }
    if hay.len() < needle.len() {
        return false;
    }

    let first = needle[0];
    let last_start = hay.len() - needle.len();
    (0..=last_start).any(|i| {
        // The candidate byte must be folded too — matching a raw 'H' against a
        // lowercased 'h' would skip every capitalised hit.
        hay[i].to_ascii_lowercase() == first
            && hay[i..i + needle.len()].eq_ignore_ascii_case(needle)
    })
}

/// Search budgets. A broad `file_actions` search used to walk the entire
/// workspace, read every file in full and buffer every matching line, which
/// turns a tree with a few thousand files into a multi-second (or worse)
/// block — even though `truncate_tool_output` keeps only 8 000 characters of
/// the result. The walk is now parallel and aborts as soon as a budget is
/// full, so a huge tree degrades to a "useful prefix" instead of a hang.
pub(super) const SEARCH_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const SEARCH_MAX_FILES: usize = 20_000;
pub(super) const SEARCH_MAX_MATCHES: usize = 1_000;
const SEARCH_MAX_ERRORS: usize = 20;

/// Directory names that hold build output or dependency caches, never project
/// source. `respect_gitignore` now defaults to true and hides most of them in a
/// git tree, but a plain folder has no `.gitignore` — and this workspace's
/// `target/` alone holds 92k files, so a default search used to spend its whole
/// budget there and return build artifacts instead of source. The check is a
/// name compare per directory (not a glob per file) and never applies to the
/// search root itself, so `path: ./target` still searches the build tree when
/// that is exactly what was asked for.
const NOISE_DIR_NAMES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "bower_components",
    "target",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".cache",
    ".parcel-cache",
    ".angular",
    ".gradle",
    "coverage",
];

fn is_noise_dir_name(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| NOISE_DIR_NAMES.contains(&name))
}

/// Why the walk stopped before finishing the tree. The footer names the reason
/// so the model narrows the query instead of retrying the same search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExhaustReason {
    MatchBudget,
    FileBudget,
}

#[derive(Clone, Debug)]
pub(super) struct SearchOptions {
    pub(super) recursive: bool,
    pub(super) case_sensitive: bool,
    pub(super) use_regex: bool,
    pub(super) smart_case: bool,
    pub(super) include_hidden: bool,
    pub(super) respect_gitignore: bool,
    pub(super) glob: Option<String>,
    /// Hard cap on collected `path:line:text` rows.
    pub(super) max_matches: usize,
    /// Set by a background task that was cancelled. The parallel walker checks
    /// it on every entry (a relaxed atomic load) and stops descending; the
    /// footer reports the early stop. `None` never aborts.
    pub(super) abort: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            recursive: true,
            case_sensitive: false,
            use_regex: false,
            smart_case: false,
            include_hidden: true,
            // On by default: a workspace is usually a git repo, and its
            // `.gitignore` is the most accurate list of "not source" that
            // exists. `false` stays available as the escape hatch.
            respect_gitignore: true,
            glob: None,
            max_matches: SEARCH_MAX_MATCHES,
            abort: None,
        }
    }
}

fn should_use_case_sensitive_search(query: &str, case_sensitive: bool, smart_case: bool) -> bool {
    case_sensitive || (smart_case && query.chars().any(|ch| ch.is_uppercase()))
}

fn normalize_search_match_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn build_search_glob_matcher(glob: Option<&str>) -> Result<Option<globset::GlobSet>, String> {
    let Some(glob) = glob.map(str::trim).filter(|glob| !glob.is_empty()) else {
        return Ok(None);
    };

    let compiled_glob = globset::GlobBuilder::new(glob)
        .literal_separator(true)
        .build()
        .map_err(|e| format!("Invalid search glob: {}", e))?;

    let mut builder = globset::GlobSetBuilder::new();
    builder.add(compiled_glob);
    builder
        .build()
        .map(Some)
        .map_err(|e| format!("Invalid search glob set: {}", e))
}

fn build_search_display_path(target: &Path, root: &Path) -> String {
    // Forward slashes on every platform: the same tree must produce the same
    // `path:line:text` rows on Windows as on macOS/Linux, otherwise the model
    // sees (and has to re-parse) backslash paths it cannot paste back in.
    normalize_search_match_path(target, root)
}

pub(super) fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(1024).any(|&byte| byte == 0)
}

/// Result of inspecting one candidate file.
enum FileScan {
    /// Nothing matched.
    Empty,
    /// Rejected by the `glob` filter before the file was opened, so it must not
    /// consume the scanned-file budget.
    FilteredOut,
    Hits(Vec<String>),
    /// Larger than `SEARCH_MAX_FILE_BYTES`; skipped without being read.
    TooLarge,
    /// Contains NUL bytes in the first KiB; skipped.
    Binary,
}

fn scan_entry(
    target: &Path,
    target_root: &Path,
    matcher: &SearchMatcher,
    glob_matcher: Option<&globset::GlobSet>,
) -> Result<FileScan, String> {
    if let Some(glob_matcher) = glob_matcher {
        let match_path = normalize_search_match_path(target, target_root);
        if !glob_matcher.is_match(&match_path) {
            return Ok(FileScan::FilteredOut);
        }
    }

    // Cheap `stat` first: a workspace full of multi-hundred-MB artifacts used
    // to be slurped into memory just to be rejected as binary afterwards.
    if let Ok(metadata) = fs::metadata(target) {
        if metadata.len() > SEARCH_MAX_FILE_BYTES {
            return Ok(FileScan::TooLarge);
        }
    }

    let bytes =
        fs::read(target).map_err(|e| format!("Failed to read '{}': {}", target.display(), e))?;

    if is_probably_binary(&bytes) {
        return Ok(FileScan::Binary);
    }

    let text = String::from_utf8_lossy(&bytes);
    let display_path = build_search_display_path(target, target_root);
    let mut hits = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if matcher.is_match(line) {
            hits.push(format!("{}:{}:{}", display_path, index + 1, line));
        }
    }

    if hits.is_empty() {
        Ok(FileScan::Empty)
    } else {
        Ok(FileScan::Hits(hits))
    }
}

/// Shared, mutex-guarded state for one search. The parallel walker runs several
/// threads at once, so the match budget is authoritative here rather than in an
/// atomic — an exact cap is worth one uncontended lock per file that has hits.
#[derive(Default)]
struct SearchAccumulator {
    results: Vec<String>,
    errors: Vec<String>,
    files_scanned: usize,
    files_glob_filtered: usize,
    files_skipped_large: usize,
    files_skipped_binary: usize,
    /// Build/dependency directories the walker refused to descend into.
    pruned_noise_dirs: usize,
    pruned_noise_names: String,
    match_budget_left: usize,
    exhausted_reason: Option<ExhaustReason>,
    /// The background task behind this search was cancelled.
    cancelled: bool,
}

impl SearchAccumulator {
    fn with_match_budget(max_matches: usize) -> Self {
        Self {
            match_budget_left: max_matches,
            ..Self::default()
        }
    }

    fn push_error(&mut self, message: String) {
        if self.errors.len() < SEARCH_MAX_ERRORS {
            self.errors.push(message);
        }
    }
}

/// Folds one file's scan into the accumulator. Returns `WalkState::Quit` once a
/// budget is spent so the walker stops descending immediately.
fn accumulate_scan(acc: &mut SearchAccumulator, scan: Result<FileScan, String>) -> WalkState {
    if acc.exhausted_reason.is_some() {
        return WalkState::Quit;
    }

    // A file the `glob` rejected is never opened, so it must not consume the
    // scanned-file budget: `glob: "**/*.md"` over a `node_modules` tree used to
    // burn all 20 000 slots on files it never looked at.
    if matches!(&scan, Ok(FileScan::FilteredOut)) {
        acc.files_glob_filtered += 1;
        return WalkState::Continue;
    }

    acc.files_scanned += 1;
    if acc.files_scanned > SEARCH_MAX_FILES {
        acc.exhausted_reason = Some(ExhaustReason::FileBudget);
        return WalkState::Quit;
    }

    match scan {
        Ok(FileScan::Hits(hits)) => {
            let take = hits.len().min(acc.match_budget_left);
            acc.match_budget_left -= take;
            acc.results.extend(hits.into_iter().take(take));
            if acc.match_budget_left == 0 {
                acc.exhausted_reason = Some(ExhaustReason::MatchBudget);
                return WalkState::Quit;
            }
        }
        Ok(FileScan::TooLarge) => acc.files_skipped_large += 1,
        Ok(FileScan::Binary) => acc.files_skipped_binary += 1,
        Ok(FileScan::Empty) | Ok(FileScan::FilteredOut) => {}
        Err(err) => acc.push_error(err),
    }

    WalkState::Continue
}

fn render_search_output(acc: &SearchAccumulator, max_matches: usize) -> String {
    if acc.results.is_empty() && acc.errors.is_empty() {
        return "(no matches)".to_string();
    }

    let mut output = acc.results.join("\n");
    if !acc.errors.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str("STDERR:\n");
        output.push_str(&acc.errors.join("\n"));
    }

    // The footer is emitted after the body so `truncate_tool_output`'s tail
    // window always keeps it: the model learns *why* the list is short and can
    // narrow the query instead of retrying the same search.
    let mut notes: Vec<String> = Vec::new();
    if acc.cancelled {
        notes.push("search cancelled before completion — results are partial".to_string());
    }
    match acc.exhausted_reason {
        Some(ExhaustReason::MatchBudget) => notes.push(format!(
            "stopped early at the {max_matches}-match limit — narrow the path, add a `glob`, or raise `max_results`"
        )),
        Some(ExhaustReason::FileBudget) => notes.push(format!(
            "stopped early after scanning {SEARCH_MAX_FILES} files — narrow the path, add a `glob`, or search a subdirectory"
        )),
        None => {}
    }
    if acc.files_skipped_large > 0 {
        notes.push(format!(
            "{} file(s) over {} MiB skipped",
            acc.files_skipped_large,
            SEARCH_MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    if acc.files_skipped_binary > 0 {
        notes.push(format!(
            "{} binary file(s) skipped",
            acc.files_skipped_binary
        ));
    }
    if acc.files_glob_filtered > 0 {
        notes.push(format!(
            "{} file(s) excluded by the `glob` filter",
            acc.files_glob_filtered
        ));
    }
    if acc.pruned_noise_dirs > 0 {
        let names = if acc.pruned_noise_names.is_empty() {
            String::new()
        } else {
            format!(" ({})", acc.pruned_noise_names)
        };
        notes.push(format!(
            "skipped {} build/dependency dir(s){} — search one explicitly to include it",
            acc.pruned_noise_dirs, names
        ));
    }
    if !notes.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&format!(
            "[search] {} file(s) scanned — {}",
            acc.files_scanned,
            notes.join("; ")
        ));
    }

    output
}

/// One budgeted parallel walk over `root`. Returns the noise directories it
/// pruned (count, distinct names) so a search that walks more than one root can
/// merge the counters.
#[allow(clippy::too_many_arguments)]
fn walk_tree(
    root: &Path,
    target_root: &Path,
    matcher: &SearchMatcher,
    glob_matcher: Option<&globset::GlobSet>,
    options: &SearchOptions,
    respect_gitignore: bool,
    acc: &Mutex<SearchAccumulator>,
    exhausted: &AtomicBool,
) -> (usize, Vec<String>) {
    let mut walker = ignore::WalkBuilder::new(root);
    walker.standard_filters(false);
    walker.hidden(!options.include_hidden);
    walker.git_ignore(respect_gitignore);
    walker.git_exclude(respect_gitignore);
    walker.parents(respect_gitignore);
    walker.ignore(respect_gitignore);
    walker.follow_links(false);

    if !options.recursive {
        walker.max_depth(Some(1));
    }

    // Prune build output and dependency caches below the search root. Without
    // this a workspace with a 92k-file `target/` spends the entire scan budget
    // there and never reaches `src/`. `filter_entry` only ever skips *more*, so
    // it cannot resurrect an entry an ignore rule already dropped.
    let pruned_noise_dirs = Arc::new(AtomicUsize::new(0));
    let pruned_noise_names = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let pruned_noise_dirs = Arc::clone(&pruned_noise_dirs);
        let pruned_noise_names = Arc::clone(&pruned_noise_names);
        walker.filter_entry(move |entry| {
            if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_dir()) {
                return true;
            }
            let name = entry.file_name();
            if !is_noise_dir_name(name) {
                return true;
            }
            pruned_noise_dirs.fetch_add(1, Ordering::Relaxed);
            let mut names = pruned_noise_names
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if names.len() < 4 {
                let name = name.to_string_lossy().to_string();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            false
        });
    }

    let abort = options.abort.as_ref();

    // Directory traversal dominates the cost of a search over a few thousand
    // files (one `stat` + one `open` + one `read` per file, all IO-bound), so
    // it is fanned out across cores. `ignore` defaults to
    // `available_parallelism().min(12)` worker threads.
    walker.build_parallel().run(|| {
        Box::new(move |entry| {
            if exhausted.load(Ordering::Relaxed)
                || abort.is_some_and(|flag| flag.load(Ordering::Relaxed))
            {
                return WalkState::Quit;
            }

            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    acc.lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push_error(err.to_string());
                    return WalkState::Continue;
                }
            };

            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                return WalkState::Continue;
            }

            // Read + match outside the lock; only the merge is serialised.
            let scan = scan_entry(entry.path(), target_root, matcher, glob_matcher);
            let state = accumulate_scan(&mut acc.lock().unwrap_or_else(|p| p.into_inner()), scan);
            if state == WalkState::Quit {
                exhausted.store(true, Ordering::Relaxed);
            }
            state
        })
    });

    let pruned = pruned_noise_dirs.load(Ordering::Relaxed);
    let names = {
        let names = pruned_noise_names
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        names.clone()
    };
    (pruned, names)
}

pub(super) fn run_integrated_search(
    query: &str,
    target: &Path,
    target_root: &Path,
    options: &SearchOptions,
) -> Result<String, String> {
    let case_sensitive =
        should_use_case_sensitive_search(query, options.case_sensitive, options.smart_case);
    let matcher = SearchMatcher::build(query, case_sensitive, options.use_regex)?;
    let glob_matcher = build_search_glob_matcher(options.glob.as_deref())?;
    let max_matches = options.max_matches.max(1);

    if target.is_file() {
        let mut acc = SearchAccumulator::with_match_budget(max_matches);
        let scan = scan_entry(target, target_root, &matcher, glob_matcher.as_ref());
        accumulate_scan(&mut acc, scan);
        return Ok(render_search_output(&acc, max_matches));
    }

    if !target.is_dir() {
        return Err(format!(
            "Search target '{}' does not exist",
            target.display()
        ));
    }

    let acc = Mutex::new(SearchAccumulator::with_match_budget(max_matches));
    // Read-mostly flag so the other worker threads bail out without first
    // serialising on the accumulator mutex.
    let exhausted = AtomicBool::new(false);

    let (mut pruned_noise_dirs, mut pruned_noise_names) = walk_tree(
        target,
        target_root,
        &matcher,
        glob_matcher.as_ref(),
        options,
        options.respect_gitignore,
        &acc,
        &exhausted,
    );

    // `.data` (database, memory, todos, skills) is part of the workspace, so the
    // assistant can look up its own skills, memories and todo lists. Repos do
    // ignore it (this one lists `.data/`), and `ignore` has no "rescue this
    // subtree" primitive — an override's plain pattern means *only include* and
    // its `!` pattern means exclude, while `filter_entry` runs after the ignore
    // rules. So when the state directory produced no rows it is walked as its
    // own root with the ignore files off: it is small, and the shared
    // accumulator keeps one match budget across both walks.
    let state_dir = target.join(".data");
    if options.respect_gitignore && options.include_hidden {
        let already_walked = acc
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .results
            .iter()
            .any(|row| row.starts_with(".data/"));
        if !already_walked && !exhausted.load(Ordering::Relaxed) && state_dir.is_dir() {
            let (dirs, names) = walk_tree(
                &state_dir,
                target_root,
                &matcher,
                glob_matcher.as_ref(),
                options,
                false,
                &acc,
                &exhausted,
            );
            pruned_noise_dirs += dirs;
            for name in names {
                if pruned_noise_names.len() < 4 && !pruned_noise_names.contains(&name) {
                    pruned_noise_names.push(name);
                }
            }
        }
    }

    let mut acc = acc
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    acc.pruned_noise_dirs = pruned_noise_dirs;
    acc.pruned_noise_names = pruned_noise_names.join(", ");
    if options
        .abort
        .as_ref()
        .is_some_and(|flag| flag.load(Ordering::Relaxed))
    {
        acc.cancelled = true;
    }
    // Parallel traversal returns rows in nondeterministic order; sort so the
    // same tree always produces byte-identical output. `dedup` collapses rows a
    // second walk could repeat.
    acc.results.sort();
    acc.results.dedup();

    Ok(render_search_output(&acc, max_matches))
}
