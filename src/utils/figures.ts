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
 * The scan is line-based and fence-aware: a figure only counts at the top
 * level, so a `svg` example inside a ```markdown block stays code. It is also
 * tolerant of a half-written block while a reply streams — an unterminated
 * figure becomes `pending` instead of flashing back to raw text.
 */

export type FigureKind = "svg" | "carve";

export interface FigureSpec {
  kind: FigureKind;
  /** Element source for `svg`, the JSON body for `carve`. */
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

// U+2063 (INVISIBLE SEPARATOR) around the index: markdown-it passes it through
// untouched, it survives the `breaks: true` line splitting, and it is
// effectively impossible to collide with real prose.
const SLOT_OPEN = "\u2063F";
const SLOT_CLOSE = "\u2063";

export const slotToken = (index: number): string => `${SLOT_OPEN}${index}${SLOT_CLOSE}`;

/** Matches one slot token, capturing the figure index. */
export const SLOT_PATTERN = /\u2063F(\d+)\u2063/g;

const FENCE_OPEN = /^ {0,3}(`{3,}|~{3,})[ \t]*([^\s`]*)[ \t]*(.*)$/;
const FENCE_CLOSE = /^ {0,3}(`{3,}|~{3,})[ \t]*$/;

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
  }
  // A carve scene may carry its own caption; the JSON is the thing the model
  // actually writes, so honour it when the fence line says nothing.
  if (kind === "carve" && (!title || !name)) {
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
 * Cut raw `<svg>` / `<carve>` elements out of one run of prose.
 *
 * A tag only counts when it is a *real* element: it must close, carry a
 * `viewBox`/size (svg) or a `{` (carve), and it must not sit inside an inline
 * code span — otherwise a sentence explaining the feature would eat itself.
 */
function extractRawTags(source: string, figures: FigureSpec[], streaming: boolean): string {
  let out = "";
  let cursor = 0;

  for (;;) {
    const open = /<(svg|carve)\b/gi.exec(source.slice(cursor));
    if (!open) break;
    const start = cursor + open.index;
    const kind = open[1].toLowerCase() as FigureKind;
    const lineStart = source.lastIndexOf("\n", start) + 1;
    // Odd number of backticks before the tag on this line → inside `code`.
    const ticks = (source.slice(lineStart, start).match(/`/g) ?? []).length;
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
    if (kind === "carve" && !body.includes("{")) {
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
    const lang = (open[2] ?? "").toLowerCase();
    if (lang === "svg" || lang === "carve") {
      const kind = lang as FigureKind;
      flushProse();
      const meta = parseFenceMeta(open[3] ?? "", body.join("\n"), kind);
      figures.push({
        kind,
        source: body.join("\n"),
        title: meta.title,
        name: meta.name,
        pending: end === -1,
      });
      out.push(slotToken(figures.length - 1));
    } else {
      // An ordinary code block is copied verbatim — and it must bypass the
      // raw-tag scan: an `svg` example inside a ```markdown block is code, so
      // it is emitted as its own chunk instead of joining the prose.
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