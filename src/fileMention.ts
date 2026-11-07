/**
 * `@path#line-line` mentions in the composer.
 *
 * Grammar (one mention, `@` must start a word):
 *
 * ```text
 * @<relative path>[#<start>[-<end>]]
 * ```
 *
 * - `@src/App.tsx`        → the whole file
 * - `@src/App.tsx#12`     → line 12 only
 * - `@src/App.tsx#12-24`  → lines 12…24
 * - `@src/App.tsx#12-`    → lines 12…EOF
 *
 * The token stays in the message body — the model then sees *which* file was
 * quoted — while the file content travels through the normal attachment
 * pipeline: text is inlined into the prompt, images/binaries become content
 * parts. Line ranges only make sense for text; a range on an image is ignored
 * (with a warning) and the whole file is attached.
 */
import { readWorkspaceFileBytes, readWorkspaceFileRange } from "./api";
import { classifyName, mimeForName, supportsAttachmentKind } from "./attachments";
import { t } from "./i18n";
import type { Attachment, ModelMetadata, WorkspaceFileEntry } from "./types";

/** `@path` optionally followed by `#start`, `#start-end` or `#start-`. */
const MENTION_RE = /@([^\s@#]+)(?:#(\d+)(?:-(\d*))?)?/g;

export interface FileMentionToken {
  /** Index of the `@` in the source text. */
  start: number;
  /** Index just past the token. */
  end: number;
  /** The whole token, e.g. `@src/App.tsx#12-24`. */
  raw: string;
  /** Workspace-relative path, e.g. `src/App.tsx`. */
  path: string;
  /** 1-based first line; `undefined` means from the top of the file. */
  startLine?: number;
  /** 1-based last line; `undefined` with `hasRange` means “to EOF”. */
  endLine?: number;
  hasRange: boolean;
}

/**
 * Find the mentions in `text`. An `@` only starts a mention at the beginning of
 * the text or after whitespace, so `user@example.com` is left alone.
 */
export function parseFileMentions(text: string): FileMentionToken[] {
  const tokens: FileMentionToken[] = [];
  for (const match of text.matchAll(MENTION_RE)) {
    const index = match.index ?? 0;
    const previous = index === 0 ? "" : text[index - 1];
    if (previous && !/\s/.test(previous)) continue;

    const raw = match[0];
    const path = match[1];
    const startText = match[2];
    const endText = match[3];
    const hasRange = startText !== undefined;

    let startLine: number | undefined;
    let endLine: number | undefined;
    if (hasRange) {
      startLine = Number(startText);
      if (endText === undefined) {
        // `#12` — a single line.
        endLine = startLine;
      } else if (endText === "") {
        // `#12-` — to the end of the file.
        endLine = undefined;
      } else {
        endLine = Number(endText);
      }
    }

    tokens.push({ start: index, end: index + raw.length, raw, path, startLine, endLine, hasRange });
  }
  return tokens;
}

/**
 * The mention token that the caret currently sits in, if any.
 *
 * Used by the composer to decide whether the `@` popup stays open: the token
 * must start at a word boundary and must not yet contain whitespace or a second
 * `@`. A `#` closes the popup — the line range is typed by hand.
 */
export function activeMentionQuery(
  text: string,
  caret: number,
): { start: number; query: string } | null {
  const before = text.slice(0, caret);
  const at = before.lastIndexOf("@");
  if (at < 0) return null;
  const previous = at === 0 ? "" : before[at - 1];
  if (previous && !/\s/.test(previous)) return null;
  const query = before.slice(at + 1);
  if (/[\s@#]/.test(query)) return null;
  return { start: at, query };
}

/** Popup ordering: file-name matches first, then path matches, then shorter paths. */
export function scoreWorkspaceFile(file: WorkspaceFileEntry, query: string): number | null {
  const needle = query.toLowerCase();
  const name = file.name.toLowerCase();
  const path = file.path.toLowerCase();
  if (needle === "") return 3;
  if (name.startsWith(needle)) return 0;
  if (name.includes(needle)) return 1;
  if (path.includes(needle)) return 2;
  return null;
}

export interface MentionResolution {
  attachments: Attachment[];
  /** Human-readable problems to surface in the error banner. */
  problems: string[];
}

/**
 * Turn the mentions in `text` into attachments.
 *
 * Failures are collected instead of thrown: a mention that cannot be resolved
 * (missing file, unsupported type, out-of-range lines) must not swallow the
 * message the user typed.
 */
export async function resolveFileMentions(
  text: string,
  metadata: ModelMetadata | undefined,
  modelLabel: string,
): Promise<MentionResolution> {
  const tokens = parseFileMentions(text);
  const attachments: Attachment[] = [];
  const problems: string[] = [];
  const seen = new Set<string>();

  for (const token of tokens) {
    const key = `${token.path}#${token.startLine ?? ""}-${token.endLine ?? ""}`;
    if (seen.has(key)) continue;
    seen.add(key);

    const kind = classifyName(token.path);
    if (!supportsAttachmentKind(metadata, kind)) {
      problems.push(t("app.attachmentUnsupported", { names: token.path, model: modelLabel }));
      continue;
    }

    try {
      if (kind === "text") {
        const range = await readWorkspaceFileRange(token.path, {
          startLine: token.startLine,
          endLine: token.endLine,
        });
        const label =
          range.start_line === range.end_line && !token.hasRange
            ? token.path
            : range.start_line === range.end_line
              ? `${token.path}#${range.start_line}`
              : `${token.path}#${range.start_line}-${range.end_line}`;
        attachments.push({
          name: label,
          kind: "text",
          mime: mimeForName(token.path),
          text_content: range.text,
        });
      } else {
        // Images/binaries are attached whole: a line range has no meaning.
        if (token.hasRange) {
          problems.push(t("app.fileMention.rangeNotSupported", { name: token.path }));
        }
        const base64 = await readWorkspaceFileBytes(token.path);
        const mime = mimeForName(token.path);
        attachments.push({
          name: token.path,
          kind,
          mime,
          data_url: `data:${mime};base64,${base64}`,
        });
      }
    } catch (err) {
      problems.push(t("app.fileMention.readFailed", { name: token.path, error: String(err) }));
    }
  }

  return { attachments, problems };
}