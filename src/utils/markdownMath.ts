import type MarkdownIt from "markdown-it";
import type StateBlock from "markdown-it/lib/rules_block/state_block.mjs";
import type StateInline from "markdown-it/lib/rules_inline/state_inline.mjs";
import katex from "katex";

type BlockState = StateBlock;
type InlineState = StateInline;

const CHAR_DOLLAR = 0x24; // '$'
const CHAR_BACKSLASH = 0x5c; // '\\'

function isWhitespaceChar(code: number): boolean {
    return (
        code === 0x20 || // space
        code === 0x09 || // tab
        code === 0x0a || // \n
        code === 0x0d || // \r
        code === 0x0b || // \v
        code === 0x0c // \f
    );
}

function isDigitChar(code: number): boolean {
    return code >= 0x30 && code <= 0x39;
}

function escapeHtml(value: string): string {
    return value
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;")
        .replace(/'/g, "&#39;");
}

function renderMath(latex: string, displayMode: boolean): string {
    try {
        return katex.renderToString(latex, {
            displayMode,
            throwOnError: false,
            strict: "ignore",
        });
    } catch {
        // KaTeX rarely throws with throwOnError disabled; keep a safe fallback.
        return escapeHtml(latex);
    }
}

/**
 * Inline rule for `$...$` (inline) and `$$...$$` (display) math.
 *
 * Delimiter validation follows the heuristics of markdown-it-katex to avoid
 * false positives on currency amounts like `$100 and $200`:
 * - an opening `$` must not be preceded by a digit and must not be followed
 *   by whitespace;
 * - a closing `$` must not be preceded by whitespace and must not be
 *   followed by a digit.
 */
function mathInline(state: InlineState, silent: boolean): boolean {
    const start = state.pos;
    const src = state.src;
    const max = state.posMax;

    if (src.charCodeAt(start) !== CHAR_DOLLAR) return false;

    const isDisplay = src.charCodeAt(start + 1) === CHAR_DOLLAR;
    const delimLen = isDisplay ? 2 : 1;

    const prevChar = start > 0 ? src.charCodeAt(start - 1) : -1;
    if (prevChar === CHAR_BACKSLASH) return false; // escaped: let the escape rule handle it

    if (!isDisplay) {
        if (start + 1 >= max) return false;
        if (isWhitespaceChar(src.charCodeAt(start + 1))) return false;
        if (isDigitChar(prevChar)) return false; // currency: `$100`
    }

    let pos = start + delimLen;
    let end = -1;
    while (pos < max) {
        const code = src.charCodeAt(pos);
        if (code === CHAR_BACKSLASH) {
            // Skip escaped characters so `\$$` / `\$` don't terminate the math.
            pos += 2;
            continue;
        }
        if (code !== CHAR_DOLLAR) {
            pos += 1;
            continue;
        }
        if (isDisplay) {
            if (src.charCodeAt(pos + 1) === CHAR_DOLLAR) {
                end = pos;
                break;
            }
            pos += 1;
            continue;
        }
        // Single `$` closing candidate.
        const prevCode = src.charCodeAt(pos - 1);
        const nextCode = pos + 1 <= max ? src.charCodeAt(pos + 1) : -1;
        if (isWhitespaceChar(prevCode)) {
            pos += 1;
            continue;
        }
        if (isDigitChar(nextCode)) {
            pos += 1;
            continue; // currency: `... $200`
        }
        end = pos;
        break;
    }

    if (end < 0) return false;

    const content = src.slice(start + delimLen, end);
    if (!content.trim()) return false;

    if (!silent) {
        const token = state.push("math_inline", "math", 0);
        token.markup = isDisplay ? "$$" : "$";
        token.content = content;
    }
    state.pos = end + delimLen;
    return true;
}

/**
 * Block rule for `$$ ... $$` spanning one or more lines:
 *
 * ```
 * $$
 * E = mc^2
 * $$
 * ```
 *
 * The delimiters may also carry content (`$$E=mc^2$$` on a single line, or
 * `$$ E=mc^2` opened and `E=mc^2$$` closed on separate lines).
 */
function mathBlock(
    state: BlockState,
    startLine: number,
    _endLine: number,
    silent: boolean,
): boolean {
    const startPos = state.bMarks[startLine] + state.tShift[startLine];
    const maxPos = state.eMarks[startLine];
    if (startPos + 2 > maxPos) return false;
    if (
        state.src.charCodeAt(startPos) !== CHAR_DOLLAR ||
        state.src.charCodeAt(startPos + 1) !== CHAR_DOLLAR
    ) {
        return false;
    }

    if (silent) return true;

    const lines: string[] = [];
    const firstLine = state.src.slice(startPos + 2, maxPos).trim();
    if (firstLine.endsWith("$$")) {
        const inner = firstLine.slice(0, -2).trim();
        if (!inner) return false;
        lines.push(inner);
    } else {
        if (firstLine) lines.push(firstLine);
        let found = false;
        let lastLine = startLine;
        for (let line = startLine + 1; line < state.lineMax; line++) {
            const pos = state.bMarks[line] + state.tShift[line];
            const text = state.src.slice(pos, state.eMarks[line]).trim();
            if (text.endsWith("$$")) {
                const inner = text.slice(0, -2).trim();
                if (inner) lines.push(inner);
                lastLine = line;
                found = true;
                break;
            }
            lines.push(text);
        }
        if (!found) return false;

        const latex = lines.join("\n").trim();
        if (!latex) return false;

        const token = state.push("math_block", "math", 0);
        token.block = true;
        token.markup = "$$";
        token.content = latex;
        token.map = [startLine, lastLine + 1];
        state.line = lastLine + 1;
        return true;
    }

    const latex = lines.join("\n").trim();
    if (!latex) return false;

    const token = state.push("math_block", "math", 0);
    token.block = true;
    token.markup = "$$";
    token.content = latex;
    token.map = [startLine, startLine + 1];
    state.line = startLine + 1;
    return true;
}

/**
 * markdown-it plugin adding KaTeX math rendering:
 * - inline math: `$...$`
 * - display math: `$$...$$` (block or inline within a paragraph)
 */
export function katexMathPlugin(md: MarkdownIt): void {
    md.inline.ruler.before("escape", "math_inline", mathInline);
    md.block.ruler.before("paragraph", "math_block", mathBlock, {
        alt: ["paragraph", "reference", "blockquote", "list"],
    });
    md.renderer.rules.math_inline = (tokens, idx) =>
        renderMath(tokens[idx].content, false);
    md.renderer.rules.math_block = (tokens, idx) =>
        renderMath(tokens[idx].content, true);
}
