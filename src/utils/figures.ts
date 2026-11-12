/**
 * Figures in a reply: the `<svg>` / `<carve>` blocks the assistant draws.
 *
 * The transcript renders markdown with `html: false` (raw HTML is the main
 * injection vector), so a `<svg>` element written by the model would reach the
 * page as escaped text. Instead of turning raw HTML on, the source is scanned
 * *before* markdown-it runs: every figure block is cut out and replaced by an
 * invisible slot token, and the parsed pieces are handed to
 * `figureCard.ts`, which mounts a real, sanitised element into the DOM after
 * `innerHTML` is committed.
 *
 * Two spellings are accepted, because models produce both:
 *
 * ```svg                    <svg viewBox="0 0 200 120">…</svg>
 * …one <svg> element…
 * ```
 *
 * ```carve                  <carve>{"objects":[…]}</carve>
 * {"objects":[…]}
 * ```
 *
 * ```flow                   {"nodes":[…],"edges":[…]}
 * {"direction":"LR","nodes":[…],"edges":[…]}
 * ```
 *
 * ```html-fig               <html-fig>…</html-fig>
 * …one self-contained HTML5 fragment (markup + style + script)…
 * ```
 *
 * The fourth kind is the dynamic one: a fragment that moves, or that the user
 * can drag. It cannot be sanitised into the transcript like an `<svg>` (a
 * drawing has no script, this one runs), so it is rendered inside a sandboxed
 * frame — see `htmlFigure.ts`. `flow` is the declarative flowchart: JSON in,
 * a laid-out, themed SVG out (see `flowDiagram.ts`) — no coordinates for the
 * model to get wrong. Extraction is identical for all of them, which is why
 * the kind lives here and not in a second scanner.
 *
 * The scan is line-based and fence-aware: a figure only counts at the top
 * level, so a `svg` example inside a ```markdown block — fenced *or* 4-space
 * indented — stays code, and so does one inside a paragraph's inline `code`
 * span (counted across lines, since a span may cross them). It is also
 * tolerant of a half-written block while a reply streams — an unterminated
 * figure becomes `pending` instead of flashing back to raw text. A reply that
 * *ends* mid-fence falls back to a plain code block once the stream is over,
 * because that is what markdown-it renders and `Drawing…` would be a lie.
 */

import { slotToken as sharedSlotToken } from "./slotTokens";

export type FigureKind = "svg" | "carve" | "flow" | "html-fig";

/**
 * Fence languages that name a figure. `htmlfig` is accepted because models drop
 * the hyphen; a plain `html` fence stays an ordinary code block on purpose — it
 * is what a reply about HTML looks like, and the transcript cannot tell it from
 * a drawing.
 */
const FIGURE_LANGS: Record<string, FigureKind> = {
  svg: "svg",
  carve: "carve",
  flow: "flow",
  "html-fig": "html-fig",
  htmlfig: "html-fig",
};

export interface FigureSpec {
  kind: FigureKind;
  /** Element source for `svg`, the JSON body for `carve`, HTML for `html-fig`. */
  source: string;
  /** Caption; empty when the model did not give one. */
  title: string;
  /** File-name stem from a `#name` token, `""` when absent. */
  name: string;
  /** The block was still open when the reply was streaming. */
  pending: boolean;
}

export interface ExtractedFigures {
  /** The markdown to render, with every figure replaced by its slot token. */
  markdown: string;
  figures: FigureSpec[];
}

/** Marker the figure slots get; `slotTokens.ts` owns the token's spelling. */
const SLOT_MARKER = "\u2063F";

const slotToken = (index: number): string => sharedSlotToken(SLOT_MARKER, index);

const FENCE_OPEN = /^ {0,3}(`{3,}|~{3,})[ \t]*([^\s`]*)[ \t]*(.*)$/;
const FENCE_CLOSE = /^ {0,3}(`{3,}|~{3,})[ \t]*$/;

/** A CommonMark indented code block: 4 spaces or a tab. */
const INDENTED_CODE = /^(?: {4}|\t)/;

/** An indented code block cannot interrupt a paragraph (CommonMark §4.4). */
const canStartIndentedCode = (prose: string[]): boolean =>
  prose.length === 0 || prose[prose.length - 1].trim() === "";

function isFenceClose(line: string, fence: string): boolean {
  const match = FENCE_CLOSE.exec(line);
  return Boolean(match) && match![1][0] === fence[0] && match![1].length >= fence.length;
}

/** `svg #parabola 抛体轨迹` → name `parabola`, title `抛体轨迹`. */
function parseFenceMeta(rest: string, body: string, kind: FigureKind): { title: string; name: string } {
  let title = rest.trim();
  let name = "";
  const named = /^#([A-Za-z0-9._-]{1,64})(?=\s|$)/.exec(title);
  if (named) {
    name = named[1];
    title = title.slice(named[0].length).trim();
  } else {
    // Models also put the token last (`flow 发布链路 #release`). Only a
    // whitespace-separated ASCII slug at the very end counts, so `C# 编译`
    // and `#话题` stay titles.
    const tail = /\s#([A-Za-z0-9._-]{1,64})$/.exec(title);
    if (tail) {
      name = tail[1];
      title = title.slice(0, tail.index).trim();
    }
  }
  // A carve scene or a flow spec may carry its own caption; the JSON is the
  // thing the model actually writes, so honour it when the fence line says
  // nothing.
  if ((kind === "carve" || kind === "flow") && (!title || !name)) {
    try {
      const scene = JSON.parse(body) as { title?: unknown; name?: unknown };
      if (!title && typeof scene.title === "string") title = scene.title.trim();
      if (!name && typeof scene.name === "string" && /^[A-Za-z0-9._-]{1,64}$/.test(scene.name)) {
        name = scene.name;
      }
    } catch {
      /* an invalid scene reports itself when it is rendered */
    }
  }
  return { title, name };
}

/**
 * Cut raw `<svg>` / `<carve>` / `<flow>` / `<html-fig>` elements out of one run
 * of prose.
 *
 * A tag only counts when it is a *real* element: it must close, carry a
 * `viewBox`/size (svg) or a `{` (carve, flow), and it must not sit inside an
 * inline code span — otherwise a sentence explaining the feature would eat
 * itself.
 */
function extractRawTags(source: string, figures: FigureSpec[], streaming: boolean): string {
  let out = "";
  let cursor = 0;

  for (;;) {
    const open = /<(svg|carve|flow|html-fig)\b/gi.exec(source.slice(cursor));
    if (!open) break;
    const start = cursor + open.index;
    const kind = open[1].toLowerCase() as FigureKind;
    // Odd number of backticks between the paragraph's start and the tag → the
    // tag sits inside an inline `code` span. The window is the whole paragraph
    // (a blank line ends it), not the line: a code span may cross lines, so
    // counting only the current line would miss one that opened above.
    const ticks = (source.slice(paragraphStartOf(source, start), start).match(/`/g) ?? []).length;
    if (ticks % 2 === 1) {
      cursor = start + open[0].length;
      continue;
    }
    const openEnd = source.indexOf(">", start);
    if (openEnd < 0) break;
    const openTag = source.slice(start, openEnd + 1);
    if (kind === "svg" && !/\b(viewBox|width|height|xmlns)\s*=/.test(openTag)) {
      cursor = openEnd + 1;
      continue;
    }
    const closeTag = `</${kind}>`;
    // Case-insensitive search without lowercasing the whole reply: `toLowerCase`
    // can change a string's length (e.g. `İ`), which would shift every index.
    const closeRe = new RegExp(closeTag, "gi");
    closeRe.lastIndex = openEnd;
    const closeMatch = closeRe.exec(source);
    const closeIdx = closeMatch ? closeMatch.index : -1;
    if (closeIdx < 0) {
      // Still streaming: show a placeholder instead of collapsing back to raw
      // text and re-flowing on the next token. Finished reply: leave it alone.
      if (streaming) {
        figures.push({
          kind,
          source: source.slice(start),
          title: attribute(openTag, "data-title") ?? attribute(openTag, "title") ?? "",
          name: attribute(openTag, "data-name") ?? "",
          pending: true,
        });
        out += `${source.slice(cursor, start)}${slotToken(figures.length - 1)}`;
        return out;
      }
      cursor = openEnd + 1;
      continue;
    }
    const body = source.slice(openEnd + 1, closeIdx);
    if ((kind === "carve" || kind === "flow") && !body.includes("{")) {
      cursor = openEnd + 1;
      continue;
    }
    // An empty `<html-fig></html-fig>` is a sentence about the feature, not a
    // drawing; a fragment has to hold something.
    if (kind === "html-fig" && !body.trim()) {
      cursor = openEnd + 1;
      continue;
    }
    out += `${source.slice(cursor, start)}${slotToken(figures.length)}`;
    figures.push({
      kind,
      source: kind === "svg" ? source.slice(start, closeIdx + closeTag.length) : body,
      title: attribute(openTag, "data-title") ?? attribute(openTag, "title") ?? "",
      name: attribute(openTag, "data-name") ?? "",
      pending: false,
    });
    cursor = closeIdx + closeTag.length;
  }

  return out + source.slice(cursor);
}

function attribute(tag: string, name: string): string | null {
  const match = new RegExp(`\\b${name}\\s*=\\s*("([^"]*)"|'([^']*)')`, "i").exec(tag);
  if (!match) return null;
  const value = (match[2] ?? match[3] ?? "").trim();
  return value || null;
}

/** Index where the paragraph containing `at` begins; a blank line ends it. */
function paragraphStartOf(source: string, at: number): number {
  let lineStart = at === 0 ? 0 : source.lastIndexOf("\n", at - 1) + 1;
  while (lineStart > 0) {
    const prevEnd = lineStart - 1; // the "\n" that ends the previous line
    const prevStart = prevEnd === 0 ? 0 : source.lastIndexOf("\n", prevEnd - 1) + 1;
    if (source.slice(prevStart, prevEnd).trim() === "") return lineStart;
    lineStart = prevStart;
  }
  return lineStart;
}

/** Split the markdown source into rendered text plus the figures it holds. */
export function extractFigures(
  markdown: string,
  opts: { streaming: boolean },
): ExtractedFigures {
  const figures: FigureSpec[] = [];
  const out: string[] = [];
  const lines = markdown.split("\n");
  let prose: string[] = [];

  const flushProse = () => {
    if (prose.length === 0) return;
    out.push(extractRawTags(prose.join("\n"), figures, opts.streaming));
    prose = [];
  };

  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const open = FENCE_OPEN.exec(line);
    if (!open) {
      // An indented code block (4+ spaces, or a tab) is code in CommonMark as
      // well: it must bypass the raw-tag scan like a fenced block does, or an
      // indented `<svg>` example becomes a figure. It only starts after a
      // blank line — an indented line inside a paragraph is a lazy
      // continuation (or list-item text), which stays prose. Verbatim
      // passthrough is always faithful: markdown-it re-parses the chunk, so
      // only the figure-vs-escaped-text decision changes.
      if (INDENTED_CODE.test(line) && canStartIndentedCode(prose)) {
        flushProse();
        const block: string[] = [];
        let j = i;
        for (; j < lines.length; j += 1) {
          const candidate = lines[j];
          if (candidate.trim() === "" || INDENTED_CODE.test(candidate)) block.push(candidate);
          else break;
        }
        // Trailing blank lines belong to the prose again.
        while (block.length > 0 && block[block.length - 1].trim() === "") {
          block.pop();
          j -= 1;
        }
        out.push(block.join("\n"));
        i = j;
        continue;
      }
      prose.push(line);
      i += 1;
      continue;
    }
    const fence = open[1];
    let end = -1;
    for (let j = i + 1; j < lines.length; j += 1) {
      if (isFenceClose(lines[j], fence)) {
        end = j;
        break;
      }
    }
    const body = lines.slice(i + 1, end === -1 ? lines.length : end);
    const kind = FIGURE_LANGS[(open[2] ?? "").toLowerCase()];
    if (kind && (end !== -1 || opts.streaming)) {
      flushProse();
      const meta = parseFenceMeta(open[3] ?? "", body.join("\n"), kind);
      figures.push({
        kind,
        source: body.join("\n"),
        title: meta.title,
        name: meta.name,
        // No closing fence yet: the card shows the pen for every kind, and an
        // `html-fig` is no exception (its frame waits for the settle commit).
        pending: end === -1,
      });
      out.push(slotToken(figures.length - 1));
    } else {
      // An ordinary code block is copied verbatim — and it must bypass the
      // raw-tag scan: an `svg` example inside a ```markdown block is code, so
      // it is emitted as its own chunk instead of joining the prose. A figure
      // fence the reply ended without closing lands here too once the stream
      // is over: markdown-it renders an unclosed fence as a code block, and a
      // card that said `Drawing…` forever would be a lie.
      flushProse();
      out.push(lines.slice(i, end === -1 ? lines.length : end + 1).join("\n"));
    }
    i = end === -1 ? lines.length : end + 1;
  }
  flushProse();
  return { markdown: out.join("\n"), figures };
}

/**
 * File-name stem for a saved figure: the model's `#name` when it is a plain
 * ASCII slug, else `fig-<n>` — the index the transcript shows in the caption.
 */
export function figureSlug(figure: FigureSpec, index: number): string {
  const name = figure.name.trim();
  if (/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(name)) return name;
  return `fig-${index + 1}`;
}
