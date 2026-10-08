// Integrated search engine for `file_actions`, `skill_read` and the memory
// store: glob/space matcher, budgeted parallel tree walk and output renderer.

use ignore::WalkState;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

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
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            recursive: true,
            case_sensitive: false,
            use_regex: false,
            smart_case: false,
            include_hidden: true,
            respect_gitignore: false,
            glob: None,
            max_matches: SEARCH_MAX_MATCHES,
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
    /// Nothing matched (also used for glob-filtered-out files).
    Empty,
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
            return Ok(FileScan::Empty);
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
    files_skipped_large: usize,
    files_skipped_binary: usize,
    match_budget_left: usize,
    exhausted: bool,
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
    if acc.exhausted {
        return WalkState::Quit;
    }

    acc.files_scanned += 1;
    if acc.files_scanned > SEARCH_MAX_FILES {
        acc.exhausted = true;
        return WalkState::Quit;
    }

    match scan {
        Ok(FileScan::Hits(hits)) => {
            let take = hits.len().min(acc.match_budget_left);
            acc.match_budget_left -= take;
            acc.results.extend(hits.into_iter().take(take));
            if acc.match_budget_left == 0 {
                acc.exhausted = true;
                return WalkState::Quit;
            }
        }
        Ok(FileScan::TooLarge) => acc.files_skipped_large += 1,
        Ok(FileScan::Binary) => acc.files_skipped_binary += 1,
        Ok(FileScan::Empty) => {}
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
    if acc.exhausted {
        notes.push(format!(
            "stopped early at the {max_matches}-match limit — narrow the path, add a `glob`, or raise `max_results`"
        ));
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

    let mut walker = ignore::WalkBuilder::new(target);
    walker.standard_filters(false);
    walker.hidden(!options.include_hidden);
    walker.git_ignore(options.respect_gitignore);
    walker.git_exclude(options.respect_gitignore);
    walker.parents(options.respect_gitignore);
    walker.ignore(options.respect_gitignore);
    walker.follow_links(false);

    if !options.recursive {
        walker.max_depth(Some(1));
    }

    let acc = Mutex::new(SearchAccumulator::with_match_budget(max_matches));
    // Read-mostly flag so the other worker threads bail out without first
    // serialising on the accumulator mutex.
    let exhausted = AtomicBool::new(false);
    let matcher = &matcher;
    let glob_matcher = glob_matcher.as_ref();
    let acc_ref = &acc;
    let exhausted_ref = &exhausted;

    // Directory traversal dominates the cost of a search over a few thousand
    // files (one `stat` + one `open` + one `read` per file, all IO-bound), so
    // it is fanned out across cores. `ignore` defaults to
    // `available_parallelism().min(12)` worker threads.
    walker.build_parallel().run(|| {
        Box::new(move |entry| {
            if exhausted_ref.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }

            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    acc_ref
                        .lock()
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
            let state =
                accumulate_scan(&mut acc_ref.lock().unwrap_or_else(|p| p.into_inner()), scan);
            if state == WalkState::Quit {
                exhausted_ref.store(true, Ordering::Relaxed);
            }
            state
        })
    });

    let mut acc = acc
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Parallel traversal returns rows in nondeterministic order; sort so the
    // same tree always produces byte-identical output.
    acc.results.sort();

    Ok(render_search_output(&acc, max_matches))
}
