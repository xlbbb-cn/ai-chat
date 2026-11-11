/**
 * Files pasted (`Ctrl+V`) or dropped onto the composer.
 *
 * The composer has no attachment channel of its own: everything the model sees
 * arrives through an `@path` mention (see `fileMention.ts`). So a pasted or
 * dropped file is first written into `<workspace>/.data/tmp/` by the backend
 * (`save_pasted_file` / `import_workspace_paths`) and the composer then inserts
 * `@.data/tmp/<name>` — the file is attached by the same code path as a mention the
 * user typed, and the token is the model's provenance for it.
 *
 * This module owns the parts that do not need Tauri: naming a clipboard blob,
 * reading it as base64, and splicing the tokens into the draft without breaking
 * the mention grammar (`@` must start a word, a token ends at whitespace).
 */
import { extensionOf, supportsMentionName } from "./attachments";
import type { ModelMetadata } from "./types";

/**
 * Largest file a paste may carry. Mirrors `MAX_PASTE_BYTES` in
 * `src-tauri/src/workspace_files.rs`: the backend is the real gate, this copy
 * only refuses before a 32 MiB base64 string crosses the IPC.
 */
export const MAX_PASTE_BYTES = 24 * 1024 * 1024;

/**
 * Extension for a clipboard payload that carries no usable name. A screenshot
 * is the common case (`image/png`), and the extension decides how the mention
 * is attached (image part vs inlined text), so it must not be guessed away.
 */
const MIME_EXTENSIONS: Record<string, string> = {
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/bmp": "bmp",
  "image/svg+xml": "svg",
  "image/tiff": "tif",
  "image/avif": "avif",
  "audio/mpeg": "mp3",
  "audio/mp3": "mp3",
  "audio/wav": "wav",
  "audio/x-wav": "wav",
  "application/pdf": "pdf",
  "application/json": "json",
  "application/zip": "zip",
  "text/plain": "txt",
  "text/markdown": "md",
  "text/csv": "csv",
  "text/html": "html",
  "text/xml": "xml",
  "application/xml": "xml",
};

/** Clipboard names that describe the transfer, not the file. */
const GENERIC_NAMES = /^(image|img|blob|file|clipboard|pasted|untitled)([-_ ]?\d+)?$/i;

/** MIME → extension (`""` when the type is unknown). */
export function extensionForMime(mime: string): string {
  const type = mime.split(";")[0]!.trim().toLowerCase();
  return MIME_EXTENSIONS[type] ?? "";
}

/**
 * Name to hand the backend for a pasted `File`.
 *
 * A file copied in Explorer already carries its real name, which must be kept —
 * it is what the user recognizes in the transcript. A screenshot arrives as
 * `image.png` (or with no name at all), so it is renamed to `screenshot.<ext>`;
 * the backend prefixes every stored name with `paste-` and its own timestamp,
 * and resolves collisions, so the name itself stays short.
 */
export function pastedFileName(file: File): string {
  const raw = (file.name ?? "").trim();
  const rawExt = extensionOf(raw);
  const ext = rawExt || extensionForMime(file.type);
  const stem = rawExt ? raw.slice(0, raw.length - rawExt.length - 1) : raw;
  const usable = raw !== "" && rawExt !== "" && !GENERIC_NAMES.test(stem);
  if (usable) return raw;

  const base = file.type.startsWith("image/") ? "screenshot" : "pasted-file";
  return ext ? `${base}.${ext}` : base;
}

/** Base64 data URL of a clipboard file (`data:<mime>;base64,…`). */
export function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(reader.error ?? new Error("the file could not be read"));
    reader.readAsDataURL(file);
  });
}

/** One file ready to be written into `<workspace>/.data/tmp/`. */
export interface PreparedPaste {
  name: string;
  dataUrl: string;
}

export interface PreparedPastes {
  ready: PreparedPaste[];
  /** Human-readable refusals (too large, empty, unreadable). */
  problems: string[];
}

/**
 * Turn the `File`s of a paste or drop into backend payloads.
 *
 * A clipboard `File` can be empty: Chromium exposes a file copied from Explorer
 * but not always its bytes, and WebView2 has no filesystem path to fall back
 * on. Such an item is reported instead of being written as a 0-byte file the
 * model would then be told to read.
 */
export async function preparePastedFiles(
  files: File[],
  formatProblem: (kind: "empty" | "tooLarge" | "unreadable", name: string, error?: string) => string,
): Promise<PreparedPastes> {
  const ready: PreparedPaste[] = [];
  const problems: string[] = [];

  for (const file of files) {
    const name = pastedFileName(file);
    if (file.size === 0) {
      problems.push(formatProblem("empty", name));
      continue;
    }
    if (file.size > MAX_PASTE_BYTES) {
      problems.push(formatProblem("tooLarge", name));
      continue;
    }
    try {
      ready.push({ name, dataUrl: await readFileAsDataUrl(file) });
    } catch (err) {
      problems.push(formatProblem("unreadable", name, String(err)));
    }
  }

  return { ready, problems };
}

/**
 * Which of `paths` the selected model can actually be shown.
 *
 * The mention pipeline classifies by extension, so the check is the same one
 * the `@` popup applies: an image needs vision, a document needs `supports_files`.
 */
export function partitionByModelSupport(
  paths: string[],
  metadata: ModelMetadata | undefined,
): { supported: string[]; unsupported: string[] } {
  const supported: string[] = [];
  const unsupported: string[] = [];
  for (const path of paths) {
    const name = path.split("/").pop() ?? path;
    if (supportsMentionName(metadata, name)) supported.push(path);
    else unsupported.push(path);
  }
  return { supported, unsupported };
}

// ─── Naming a mention in a message ──────────────────────────────────────────

/** Workspace-relative scratch folder, addressed by the short `@@<name>` form. */
const TMP_REL = ".data/tmp";

/** Is this workspace-relative path a file inside the workspace tmp folder? */
export function isTmpPath(path: string): boolean {
  return path === TMP_REL || path.startsWith(`${TMP_REL}/`);
}

/**
 * The name a mention is *shown* by.
 *
 * A file in `.data/tmp` is addressed as `@@<name>`: that folder *is* the
 * workspace tmp area (composer pastes, sub-agent deliverables, the PDF cache),
 * so the short form is honest about the lifetime and keeps
 * `.data/tmp/qa-items.json` out of every message. A real path in the body also
 * reads to the model as "open this file", which is the tool call the inlined
 * copy exists to avoid.
 *
 * Every other mention keeps its path, because there the path *is* the point:
 * the model may need the rest of the file, or may write back to it.
 */
export function mentionDisplayName(path: string): string {
  const slash = path.lastIndexOf("/");
  const base = path.slice(slash + 1);
  const hash = base.indexOf("#");
  const suffix = hash >= 0 ? base.slice(hash) : "";
  if (!isTmpPath(path)) return path;
  return `@@${hash >= 0 ? base.slice(0, hash) : base}${suffix}`;
}

/** `@.data/tmp/<name>` plus the optional `#start[-end]` range of `MENTION_RE`. */
const LONG_TMP_MENTION_RE = /(^|\s)@\.data\/tmp\/([^\s@#]+)(?:#(\d+)(?:-(\d*))?)?/g;

/**
 * Collapse a hand-typed `@.data/tmp/<name>` to the short `@@<name>` form.
 *
 * The composer inserts `@@<name>` itself (and `parseFileMentions` resolves it),
 * so this only catches the long form: typed by hand, or carried in a message
 * written before the `@@` syntax existed. The range survives in the app's own
 * `#12-24` shape — the inlined copy holds only those lines, so dropping it would
 * tell the model it saw the whole file — and the boundary rule is the parser's
 * (`@` at the start or after whitespace), so `user@example.com` is left alone.
 */
export function canonicalTmpMentions(text: string): string {
  return text.replace(
    LONG_TMP_MENTION_RE,
    (_match, lead: string, name: string, start?: string, end?: string) => {
      const range =
        start === undefined ? "" : end === undefined ? `#${start}` : `#${start}-${end}`;
      return `${lead}@@${name}${range}`;
    },
  );
}

/**
 * The token for a workspace-relative path: `@@<name>` inside `.data/tmp`,
 * `@<path>` everywhere else.
 *
 * `@@` keeps a paste short (`@@qa-items.json`, not `@.data/tmp/qa-items.json`)
 * and says what the file is — scratch, not part of the project — while
 * `parseFileMentions` maps it straight back to `.data/tmp/<name>`.
 */
export function mentionTokenForPath(path: string): string {
  if (!isTmpPath(path) || path === TMP_REL) return `@${path}`;
  return `@@${path.slice(TMP_REL.length + 1)}`;
}

/**
 * Splice `tokens` into `draft` at `caret`.
 *
 * A mention only counts where an `@` starts a word, so a leading space is added
 * when the caret follows text; a trailing space keeps whatever was already
 * typed after the caret from running into the last token. The returned caret
 * sits right after the last token, so a `#12-24` range can be typed straight
 * away (same contract as accepting an item from the `@` popup).
 */
export function insertMentionTokens(
  draft: string,
  caret: number,
  tokens: string[],
): { value: string; caret: number } {
  if (tokens.length === 0) return { value: draft, caret };
  const at = Math.max(0, Math.min(caret, draft.length));
  const before = draft.slice(0, at);
  const after = draft.slice(at);
  const lead = before.length === 0 || /\s$/.test(before) ? "" : " ";
  const body = tokens.join(" ");
  const trail = after.length === 0 || /^\s/.test(after) ? "" : " ";
  return {
    value: `${before}${lead}${body}${trail}${after}`,
    caret: before.length + lead.length + body.length,
  };
}
