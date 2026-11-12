/**
 * Fence scanning shared by the two extractors.
 *
 * `figures.ts` (svg / carve / flow / html-fig) and `mermaidDiagrams.ts` cut a
 * block out of a reply *before* markdown-it runs, so both have to agree on what
 * counts as a fence — one copy, so a fix cannot land in one file and be
 * forgotten in the other.
 *
 * Two shapes bite in practice:
 *
 * * a reply that arrives with CRLF line endings. `\r` is not part of a closing
 *   fence, so every block would look unclosed and fall back to a code block once
 *   the stream is over; `splitLines` normalises them first.
 * * a fence indented *inside a list item*. A model that indents its blocks by a
 *   fixed four spaces puts the opening fence past CommonMark's three-space
 *   limit, where markdown-it still reads a fence (the list item supplies the
 *   container indentation) but a flat scan sees an indented code block — and the
 *   diagram stays code.
 */

export interface FenceOpening {
  /** The marker itself, e.g. ``` or ~~~. */
  fence: string;
  /** Lower-cased first word of the info string; empty for a bare fence. */
  lang: string;
  /** What follows the language on the fence line: a caption, a name. */
  rest: string;
  /** Leading spaces on the fence line. */
  indent: number;
}

/** CommonMark lets a fence sit under three spaces of indentation. */
const MAX_TOP_LEVEL_INDENT = 3;

/** Deeper than that it is an indented code block — unless a list item holds it. */
const MAX_LIST_INDENT = 8;

const FENCE = /^( *)(`{3,}|~{3,})[ \t]*([^\s`]*)[ \t]*(.*)$/;
const FENCE_CLOSE = /^ {0,8}(`{3,}|~{3,})[ \t]*$/;
const LIST_ITEM = /^( *)(?:[-*+]|\d{1,9}[.)])(?:[ \t]+|$)/;

/** A CommonMark indented code block: four spaces or a tab. */
export const INDENTED_CODE = /^(?: {4}|\t)/;

/** Split into lines the way the extractors scan them: LF only. */
export function splitLines(source: string): string[] {
  return source.replace(/\r\n?/g, "\n").split("\n");
}

export function readFence(line: string): FenceOpening | null {
  const match = FENCE.exec(line);
  if (!match) return null;
  return {
    fence: match[2],
    lang: match[3].toLowerCase(),
    rest: match[4],
    indent: match[1].length,
  };
}

export function isFenceClose(line: string, fence: string): boolean {
  const match = FENCE_CLOSE.exec(line);
  return Boolean(match) && match![1][0] === fence[0] && match![1].length >= fence.length;
}

/**
 * Does this opening line really start a fence, or is it an indented code block?
 *
 * Under three spaces it is always a fence. Deeper, only an enclosing list item
 * makes it one — that is how markdown-it reads it, and it is the only indented
 * shape worth honouring: a diagram the model nested in a list.
 */
export function fenceIsOpen(open: FenceOpening, lines: string[], index: number): boolean {
  if (open.indent <= MAX_TOP_LEVEL_INDENT) return true;
  if (open.indent > MAX_LIST_INDENT) return false;
  return insideListItem(lines, index, open.indent);
}

function insideListItem(lines: string[], index: number, indent: number): boolean {
  for (let i = index - 1; i >= 0; i -= 1) {
    const line = lines[i];
    if (line.trim() === "") continue;
    const item = LIST_ITEM.exec(line);
    if (item && item[1].length < indent) return true;
    // A dedented line that opens no item means the list is behind us.
    if (!item && line.length - line.trimStart().length < indent) return false;
  }
  return false;
}

/**
 * Take the container indentation off a block's body: the diagram renderer is
 * handed the source the author wrote, not the list margin around it.
 */
export function stripIndent(text: string, indent: number): string {
  if (indent <= 0) return text;
  return text.replace(new RegExp(`^ {0,${indent}}`, "gm"), "");
}

/**
 * Where the slot token goes: the fence's own column, capped at three spaces so
 * markdown-it never reads the token as an indented code block. Inside a list
 * item that keeps the card in the item; anywhere else it is the same line a
 * top-level fence leaves behind.
 */
export function slotIndent(indent: number): string {
  return " ".repeat(Math.min(indent, MAX_TOP_LEVEL_INDENT));
}
