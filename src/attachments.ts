/**
 * File-kind classification shared by the attachment picker (paperclip button)
 * and the composer's `@file` mention popup.
 *
 * The picker works from a `File` (so it can trust the browser's MIME sniffing);
 * a mention only knows a workspace path, so it classifies by extension. Both
 * paths end in the same `Attachment["kind"]` taxonomy, which is what the model
 * capabilities are expressed in (`supportsAttachmentKind`).
 */
import type { Attachment, ModelMetadata } from "./types";

/**
 * Which extensions the picker offers depends on the selected model's
 * capabilities (see `supportsAttachmentKind`):
 * - text is always accepted — it is inlined into the prompt, not sent as a
 *   content part, so any model can consume it;
 * - images become `image_url` parts → need `supports_vision`;
 * - documents become `file` parts → need `supports_files`;
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
  "jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "heic", "heif", "ico", "tif", "tiff",
];
export const FILE_EXTENSIONS = ["pdf"];
export const AUDIO_EXTENSIONS = ["mp3", "wav"];

const FILE_TYPE_EXT_OVERRIDES: Record<string, string> = {
  pdf: "application/pdf",
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
 */
export function classifyName(name: string): Attachment["kind"] {
  const ext = extensionOf(name);
  if (!ext) return "file";
  if (IMAGE_EXTENSIONS.includes(ext)) return "image";
  if (AUDIO_EXTENSIONS.includes(ext)) return "audio";
  if (TEXT_EXTENSIONS.includes(ext)) return "text";
  return "file";
}

/** MIME type derived from the extension (`application/octet-stream` when unknown). */
export function mimeForName(name: string): string {
  const ext = extensionOf(name);
  if (!ext) return "application/octet-stream";
  if (ext === "jpg" || ext === "jpeg") return "image/jpeg";
  if (FILE_TYPE_EXT_OVERRIDES[ext]) return FILE_TYPE_EXT_OVERRIDES[ext];
  if (TEXT_EXTENSIONS.includes(ext)) return ext === "md" || ext === "markdown" ? "text/markdown" : "text/plain";
  if (IMAGE_EXTENSIONS.includes(ext)) return `image/${ext === "svg" ? "svg+xml" : ext}`;
  if (AUDIO_EXTENSIONS.includes(ext)) return `audio/${ext}`;
  return `application/${ext}`;
}

/**
 * Can the model behind `metadata` consume this kind of attachment?
 *
 * Unknown metadata (the catalogue was never fetched, or the id was not matched)
 * stays permissive: blocking files a model might well accept is worse than
 * letting the provider answer with its own error.
 */
export function supportsAttachmentKind(
  metadata: ModelMetadata | undefined,
  kind: Attachment["kind"],
): boolean {
  if (!metadata) return true;
  switch (kind) {
    case "text":
      return true;
    case "image":
      return metadata.supports_vision === true;
    case "audio":
      return metadata.supports_audio === true;
    case "file":
      return metadata.supports_files === true;
  }
}