/**
 * `@path#range` and `@@tmp-name#range` mentions in the composer.
 *
 * Grammar (one mention, `@` must start a word):
 *
 * ```text
 * @<relative path>[#<start>[-<end>]]
 * @@<path inside .data/tmp>[#<start>[-<end>]]
 * ```
 *
 * The double `@` addresses the workspace tmp folder, which is what makes a
 * pasted file's token short — `@@qa-items.json` instead of
 * `@.data/tmp/qa-items.json` — and says the file is scratch rather than project
 * content. It resolves to exactly the same path, so everything below (ranges,
 * PDF pages, images) behaves identically:
 *
 * - `@@qa-items.json`      → `.data/tmp/qa-items.json`
 * - `@@qa-items.json#5-9`  → the same file, lines 5…9
 * - `@@20261111-155012-ab/shared/out.md` → a sub-agent deliverable
 *
 * Text files count *lines*:
 *
 * - `@src/App.tsx`        → the whole file
 * - `@src/App.tsx#12`     → line 12 only
 * - `@src/App.tsx#12-24`  → lines 12…24
 * - `@src/App.tsx#12-`    → lines 12…EOF
 *
 * A PDF counts *pages*, because its text is not addressable by line:
 *
 * - `@paper.pdf#2`        → page 2, converted to Markdown
 * - `@paper.pdf#2-5`      → pages 2…5
 * - `@paper.pdf#2-`       → page 2 to the end
 *
 * The token stays in the message body — the model then sees *which* file was
 * quoted — while the content travels through the normal attachment pipeline:
 * text is inlined into the prompt, images/audio become content parts. A PDF
 * becomes text too: the backend renders the page range to Markdown
 * (`convert_pdf_mention`), caches it under `.data/tmp/pdf/` and the Markdown is
 * inlined — so a PDF mention works with any model, no `supports_files` needed.
 * A range on an image is ignored (with a warning) and the whole file is
 * attached.
 */
import { convertPdfMention, readWorkspaceFileBytes, readWorkspaceFileRange } from "./api";
import type { PdfMentionResult } from "./api";
import { classifyName, extensionOf, mimeForName, supportsAttachmentKind } from "./attachments";
import { t } from "./i18n";
import type { Attachment, ModelMetadata, WorkspaceFileEntry } from "./types";

/** `.data/tmp`, the folder the `@@` form is relative to. */
const TMP_REL = ".data/tmp";

/** `@path` / `@@tmp-name`, optionally followed by `#start`, `#start-end` or `#start-`. */
const MENTION_RE = /(@@?)([^\s@#]+)(?:#(\d+)(?:-(\d*))?)?/g;

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
    // `@@name` is the short form of `.data/tmp/name`; both become one path.
    const path = match[1] === "@@" ? `${TMP_REL}/${match[2]}` : match[2];
    const startText = match[3];
    const endText = match[4];
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
  // A second `@` means the `@@` tmp form: it is typed (or inserted), never
  // completed from the workspace listing, so the popup stays closed.
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
 * Live progress of the PDF conversions inside one {@link resolveFileMentions}
 * run.
 *
 * A 60-page mention spends seconds in the backend before the turn even starts,
 * so the composer renders these ticks ("converting kk.pdf — page 3 of 30")
 * while it waits. `page`/`total` are 0 for the stages that do not walk pages
 * (reading, parsing, rendering); `percent` covers the whole conversion and never
 * walks backwards, which is what a progress bar needs.
 */
export interface MentionProgress {
  /** Workspace path of the PDF being converted right now. */
  name: string;
  /**
   * Pipeline stage code: `reading`, `extracting`, `analysing`, `parsing`,
   * `cleaning`, `rendering` or `done`.
   */
  stage: string;
  /** Page being worked on; 0 outside the page stages. */
  page: number;
  /** Pages in the requested range; 0 outside the page stages. */
  total: number;
  /** Overall completion, 0–100. */
  percent: number;
}

/**
 * `true` when the mention names a PDF.
 *
 * A PDF is the one binary the composer can turn into *text*: sending it as a
 * `file` part needs a model with `supports_files`, while its Markdown works
 * everywhere and is far cheaper to read.
 */
function isPdfMention(path: string): boolean {
  return extensionOf(path) === "pdf";
}

/**
 * Attachment label for a converted PDF, matching the token the user typed but
 * with the pages that were actually converted (the range may have been
 * clamped to the document).
 */
function pdfLabel(path: string, result: PdfMentionResult): string {
  return result.start_page === result.end_page
    ? `${path}#${result.start_page}`
    : `${path}#${result.start_page}-${result.end_page}`;
}

/**
 * Turn the mentions in `text` into attachments.
 *
 * Failures are collected instead of thrown: a mention that cannot be resolved
 * (missing file, unsupported type, out-of-range lines) must not swallow the
 * message the user typed.
 *
 * `onProgress` receives the conversion ticks of every PDF in the text, tagged
 * with the path it belongs to: resolving happens before the turn starts, so
 * without it a long document is a silent pause.
 */
export async function resolveFileMentions(
  text: string,
  metadata: ModelMetadata | undefined,
  modelLabel: string,
  onProgress?: (progress: MentionProgress) => void,
): Promise<MentionResolution> {
  const tokens = parseFileMentions(text);
  const attachments: Attachment[] = [];
  const problems: string[] = [];
  const seen = new Set<string>();

  for (const token of tokens) {
    const key = `${token.path}#${token.startLine ?? ""}-${token.endLine ?? ""}`;
    if (seen.has(key)) continue;
    seen.add(key);

    // A PDF mention is a *text* attachment: the pages are rendered to Markdown
    // by the backend and cached, so no model capability is involved and the
    // provider never sees a file part.
    if (isPdfMention(token.path)) {
      try {
        const result = await convertPdfMention(
          token.path,
          {
            startPage: token.startLine,
            endPage: token.endLine,
          },
          (update) => onProgress?.({ name: token.path, ...update }),
        );
        attachments.push({
          name: pdfLabel(token.path, result),
          kind: "text",
          mime: "text/markdown",
          text_content: result.markdown,
          // The Markdown also goes into the message `content`; the stored
          // attachment keeps only this path (see `compactAttachmentsForDb`),
          // which is what the provenance comment points at too.
          path: result.path,
        });
      } catch (err) {
        problems.push(t("app.fileMention.pdfFailed", { name: token.path, error: String(err) }));
      }
      continue;
    }

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
