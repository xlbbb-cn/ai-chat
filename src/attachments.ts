/**
 * File-kind classification for the composer's `@file` mention popup and for a
 * pasted/dropped file (both arrive as a workspace path — see `composerFiles.ts`).
 *
 * A mention only knows a name, so the kind comes from the extension; the
 * payload itself has the last word at send time (`partsForContent` in
 * `messageParts.ts` turns any `data:image/…` payload into an `image_url` part
 * whatever the extension said).
 */
import type { Attachment, ModelMetadata } from "./types";

/**
 * Which extension maps to which kind, and therefore which content part a file
 * becomes. Which of those the selected model accepts is
 * `supportsAttachmentKind`'s job:
 * - text is always accepted — it is inlined into the prompt, not sent as a
 *   content part, so any model can consume it;
 * - images become `image_url` parts → need `supports_vision`;
 * - binaries have no sendable form at all — the app never emits a `file` part, so
 *   they are refused here (see the `file`-part gotcha in
 *   `docs/agents/workflow.md`: a gateway rejects that part whatever the
 *   catalogue advertises);
 * - audio becomes an `input_audio` part → needs `supports_audio`, and only the
 *   two formats the Chat Completions schema allows (`wav`, `mp3`).
 */
export const TEXT_EXTENSIONS = [
  "txt", "md", "markdown", "csv", "tsv", "json", "jsonl",
  "yaml", "yml", "xml", "html", "htm", "css", "js", "ts", "jsx", "tsx",
  "py", "rs", "go", "java", "c", "cpp", "h", "hpp", "cs", "rb", "php",
  "sh", "bat", "ps1", "sql", "log", "ini", "cfg", "toml", "env",
  "diff", "patch", "tex", "rst", "adoc", "org", "r", "m", "scala",
  "swift", "kt", "dart", "lua", "pl", "ex", "exs", "clj", "hs", "ml",
  "fs", "erl", "vim", "conf", "v",
];
export const IMAGE_EXTENSIONS = [
  "jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "heic", "heif", "ico",
  "tif", "tiff", "avif", "apng", "jxl", "jfif",
];
export const AUDIO_EXTENSIONS = ["mp3", "wav"];

/**
 * Extensions that stay binary: a mention of one can only ever be a `file`
 * part, because inlining its decoded bytes would put mojibake in the prompt.
 *
 * Everything *not* in this list and not in {@link IMAGE_EXTENSIONS} /
 * {@link AUDIO_EXTENSIONS} is treated as text (see {@link classifyName}), which
 * is what keeps `Dockerfile`, `.gitignore`, `LICENSE` and the long tail of
 * config formats out of the `file` branch — a coding assistant is asked about
 * those far more often than about a `.docx`.
 */
export const BINARY_EXTENSIONS = [
  // Documents and containers
  "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp",
  "rtf", "epub", "mobi", "azw3",
  // Archives and executables
  "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "xz", "zst", "lz4",
  "exe", "dll", "so", "dylib", "bin", "o", "obj", "a", "lib", "class",
  "jar", "war", "wasm", "iso", "dmg", "msi", "apk", "ipa",
  // Data stores and media
  "db", "sqlite", "sqlite3", "parquet", "arrow", "feather", "pkl", "npy",
  "npz", "h5", "mp4", "webm", "mov", "mkv", "avi", "m4v", "flac", "m4a",
  "ogg", "aac", "ttf", "otf", "woff", "woff2", "eot", "psd",
];

const FILE_TYPE_EXT_OVERRIDES: Record<string, string> = {
  pdf: "application/pdf",
};

/** MIME types whose extension is not simply `image/<ext>`. */
const IMAGE_MIME_OVERRIDES: Record<string, string> = {
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  svg: "image/svg+xml",
  tif: "image/tiff",
  tiff: "image/tiff",
  ico: "image/x-icon",
};

/** Lowercase extension without the dot (`""` when the name has none). */
export function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot >= 0 ? name.slice(dot + 1).toLowerCase() : "";
}

/**
 * Classify a workspace file by its name alone (the `@` mention case).
 *
 * Slightly more permissive than the picker's MIME-first classification on
 * purpose: a `.ts` file the OS would label `video/mp2t` must still be text.
 *
 * An unknown extension — and every file without one, which covers
 * `Dockerfile`, `Makefile`, `LICENSE` and dotfiles such as `.gitignore` — is
 * `text`. The read step refuses a payload that decodes to binary
 * ({@link looksLikeBinaryText}), so this default can never smuggle bytes into
 * the prompt, while it keeps ordinary source mentions out of the `file`
 * branch: `file` parts are what providers reject.
 */
export function classifyName(name: string): Attachment["kind"] {
  const ext = extensionOf(name);
  if (!ext) return "text";
  if (IMAGE_EXTENSIONS.includes(ext)) return "image";
  if (AUDIO_EXTENSIONS.includes(ext)) return "audio";
  if (TEXT_EXTENSIONS.includes(ext)) return "text";
  if (BINARY_EXTENSIONS.includes(ext)) return "file";
  return "text";
}

/** MIME type derived from the extension (`text/plain` when unknown). */
export function mimeForName(name: string): string {
  const ext = extensionOf(name);
  if (!ext) return "text/plain";
  if (FILE_TYPE_EXT_OVERRIDES[ext]) return FILE_TYPE_EXT_OVERRIDES[ext];
  if (IMAGE_EXTENSIONS.includes(ext)) return IMAGE_MIME_OVERRIDES[ext] ?? `image/${ext}`;
  if (AUDIO_EXTENSIONS.includes(ext)) return `audio/${ext}`;
  if (BINARY_EXTENSIONS.includes(ext)) return `application/${ext}`;
  return ext === "md" || ext === "markdown" ? "text/markdown" : "text/plain";
}

/**
 * Does text decoded from a workspace file look like binary rather than text?
 *
 * `read_workspace_file_range` decodes with `String::from_utf8_lossy`, so binary
 * bytes arrive as U+FFFD replacement characters. {@link classifyName} treats an
 * unknown extension as text; this is the check that refuses the payload when
 * that guess was wrong, instead of inlining mojibake into the prompt (or, worse,
 * wrapping the bytes in a `file` part the provider rejects).
 */
export function looksLikeBinaryText(text: string): boolean {
  if (text.includes("\u0000")) return true;
  const sample = text.length > 4000 ? text.slice(0, 4000) : text;
  if (sample.length === 0) return false;
  let bad = 0;
  for (const ch of sample) {
    if (ch === "\uFFFD") bad += 1;
  }
  return bad / sample.length > 0.02;
}

/**
 * Can the model behind `metadata` consume this kind of attachment?
 *
 * Unknown metadata (the catalogue was never fetched, or the id was not matched)
 * stays permissive for the *media* kinds: blocking an image or an audio clip a
 * model may well accept is worse than letting the provider answer with its own
 * error. A `file` part is the exception — see below.
 */
export function supportsAttachmentKind(
  metadata: ModelMetadata | undefined,
  kind: Attachment["kind"],
): boolean {
  // Text is inlined into the prompt instead of becoming a content part, so it
  // never depends on the model.
  if (kind === "text") return true;
  // Binary attachments have no sendable form at all. The app emits text,
  // `image_url` and `input_audio` parts and nothing else: a `file` part is
  // rejected outright ("file content parts are not supported here") by a
  // text-and-image gateway *even when the catalogue advertises file input*,
  // which is what made a session fail on every turn after one `@scripts/x`.
  if (kind === "file") return false;
  // Vision and audio stay permissive without metadata: refusing an image a
  // model may well accept is worse than letting the provider answer itself.
  if (!metadata) return true;
  switch (kind) {
    case "image":
      return metadata.supports_vision === true;
    case "audio":
      return metadata.supports_audio === true;
  }
}

/**
 * Can the composer offer `name` as an `@` mention to this model?
 *
 * Same rule as {@link supportsAttachmentKind} with one exception: a PDF
 * mention is *converted to Markdown text* by the backend (`convert_pdf_mention`),
 * not sent as a `file` part, so it is available to every model — a model
 * without `supports_files` can still discuss page 2 of a paper.
 */
export function supportsMentionName(
  metadata: ModelMetadata | undefined,
  name: string,
): boolean {
  if (extensionOf(name) === "pdf") return true;
  return supportsAttachmentKind(metadata, classifyName(name));
}

/**
 * Attachments as they are written to the session history.
 *
 * A converted PDF mention carries the Markdown twice on the way out: once as
 * `text_content` (what `partsForContent` inlines into the turn) and once as
 * the path of its cache file. The Markdown also lands in the message `content`,
 * which is what every later request replays — so the copy inside the attachment
 * record is up to 60 000 characters of dead weight per mention. Storing the
 * path instead keeps the provenance (and the file a tool can read) at the cost
 * of one string.
 *
 * Live attachments keep their `text_content`: only the persisted copy is
 * compacted, so sending never depends on a file that may since have been
 * pruned. Attachments without a `path` (an `@notes.md#12` slice, which is read
 * from the workspace on demand) are stored unchanged.
 */
export function compactAttachmentsForDb(attachments: Attachment[]): Attachment[] {
  return attachments.map((att) => {
    if (att.text_content === undefined || !att.path) return att;
    const compact: Attachment = { ...att };
    delete compact.text_content;
    return compact;
  });
}
