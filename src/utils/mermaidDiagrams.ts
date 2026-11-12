/**
 * Mermaid diagrams in a reply.
 *
 * The plain drawing mode (`enhanced_figures` off) asks the model for a fenced
 * `mermaid` block instead of hand-written `<svg>` / `carve` markup. Rendering
 * follows the figure pipeline in `figures.ts` / `figureCard.ts`, for the same
 * reason: the transcript renders markdown with `html: false`, so a block is cut
 * out *before* markdown-it runs and replaced by a slot token, and
 * `mountDiagrams` swaps each token for a card once `innerHTML` is committed.
 *
 * Two differences from a drawn figure:
 *
 * * rendering is asynchronous (Mermaid is a lazily imported chunk) and its
 *   result is an `<svg>` element, so a card paints itself in place instead of
 *   being built synchronously;
 * * Mermaid bakes its colours into the SVG, so a theme or palette switch has to
 *   re-render, not just restyle. `watchTheme` is the same observer the 3D cards
 *   use.
 *
 * Safety: Mermaid source is model text. `securityLevel: "strict"` and
 * `htmlLabels: false` keep labels as escaped `<text>`, and `pruneMermaidSvg`
 * drops whatever HTML the renderer may still emit (scripts, `foreignObject`,
 * event handlers, external `url()`). Mermaid's own `<style>` is kept — it is
 * scoped to the `<svg>` id it generated — because without it a diagram has no
 * look at all, and it is not reachable from reply text.
 */

import { t as text } from "../i18n";
import { showUnrenderable, watchTheme, readToken } from "./figureCard";
import {
  INDENTED_CODE,
  fenceIsOpen,
  isFenceClose,
  readFence,
  slotIndent,
  splitLines,
  stripIndent,
} from "./fenceLines";
import { expandSlots, hashSource, slotHost, slotToken } from "./slotTokens";

export interface DiagramSpec {
  /** Mermaid source, without the fences. */
  source: string;
  /** Caption from the fence line; empty when the model did not give one. */
  title: string;
  /** The block was still open while the reply was streaming. */
  pending: boolean;
}

export interface ExtractedDiagrams {
  markdown: string;
  diagrams: DiagramSpec[];
}

/** Marker for a diagram slot; `slotTokens.ts` owns the token's spelling. */
const SLOT_MARKER = "\u2063M";
const SLOT_OPTIONS = { marker: SLOT_MARKER, className: "chat-mermaid-slot", datasetKey: "mermaidSlot" };

/**
 * Cut `mermaid` fences out of rendered markdown, leaving a slot token behind.
 *
 * Fence-aware and indentation-aware like `extractFigures` (both use
 * `fenceLines.ts`): a `mermaid` example inside another fence or a 4-space
 * indented block stays code, while a block a list item encloses is a diagram —
 * and is dedented before Mermaid sees it. An unclosed
 * fence becomes a `pending` card while the reply streams (so the block does not
 * flash back to raw text on every token) and an ordinary code block once it is
 * over — markdown-it renders an unclosed fence as code, and a card stuck on
 * `Drawing…` would be a lie.
 */
export function extractDiagrams(source: string, options: { streaming: boolean }): ExtractedDiagrams {
  const lines = splitLines(source);
  const out: string[] = [];
  const diagrams: DiagramSpec[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];
    const opening = readFence(line);
    const open = opening && fenceIsOpen(opening, lines, i) ? opening : null;
    if (open) {
      const fence = open.fence;
      let end = -1;
      for (let j = i + 1; j < lines.length; j += 1) {
        if (isFenceClose(lines[j], fence)) {
          end = j;
          break;
        }
      }
      const lang = open.lang;
      if (lang === "mermaid" && (end !== -1 || options.streaming)) {
        const body = stripIndent(
          lines.slice(i + 1, end === -1 ? lines.length : end).join("\n"),
          open.indent,
        );
        diagrams.push({
          source: body.trim(),
          title: open.rest.trim(),
          pending: end === -1,
        });
        out.push(slotIndent(open.indent) + slotToken(SLOT_MARKER, diagrams.length - 1));
      } else {
        out.push(...lines.slice(i, end === -1 ? lines.length : end + 1));
      }
      i = end === -1 ? lines.length : end + 1;
      continue;
    }
    // An indented code block only starts after a blank line (CommonMark §4.4),
    // and a `mermaid` line inside one is literal text, not a diagram.
    if (
      INDENTED_CODE.test(line) &&
      line.trim() !== "" &&
      (i === 0 || lines[i - 1].trim() === "")
    ) {
      while (i < lines.length && (INDENTED_CODE.test(lines[i]) || lines[i].trim() === "")) {
        out.push(lines[i]);
        i += 1;
      }
      continue;
    }
    out.push(line);
    i += 1;
  }
  return { markdown: out.join("\n"), diagrams };
}

// ─── Mermaid ────────────────────────────────────────────────────────────────

/** The slice of Mermaid's API this module uses, so the lazy import stays typed. */
interface MermaidApi {
  initialize: (config: Record<string, unknown>) => void;
  parse: (source: string, options?: { suppressErrors?: boolean }) => boolean | Promise<boolean>;
  render: (id: string, source: string) => Promise<{ svg: string }>;
}

let loader: Promise<MermaidApi> | null = null;

/** Load Mermaid once; it is a separate chunk and stays off the startup path. */
function loadMermaid(): Promise<MermaidApi> {
  if (!loader) {
    loader = import("mermaid").then(
      (module) => (module.default ?? module) as unknown as MermaidApi,
    );
  }
  return loader;
}

// Mermaid keeps global state (`initialize`, a temp container per render), so
// renders are serialised instead of racing each other on a streamed reply.
let queue: Promise<unknown> = Promise.resolve();

function enqueue<T>(task: () => Promise<T>): Promise<T> {
  const run = queue.then(task, task);
  queue = run.then(
    () => undefined,
    () => undefined,
  );
  return run;
}

interface DiagramTheme {
  surface: string;
  panel: string;
  text: string;
  muted: string;
  accent: string;
  border: string;
  font: string;
}

/** Light values the printed page declares, for a diagram re-rendered for it. */
const PRINT_THEME: DiagramTheme = {
  surface: "#ffffff",
  panel: "#f6f7f8",
  text: "#171a20",
  muted: "#5c5e62",
  accent: "#3e6ae1",
  border: "#d9dce1",
  font: '"RazerF5", "Segoe UI", "Microsoft YaHei", sans-serif',
};

/** The card's live tokens; a diagram is drawn with what the interface uses. */
function readTheme(host: Element): DiagramTheme {
  const computed = window.getComputedStyle(host);
  const font = computed.fontFamily.trim();
  return {
    surface: readToken(host, "--c-surface", "#ffffff"),
    panel: readToken(host, "--c-surface-2", "#f4f4f4"),
    text: readToken(host, "--c-text", "#171a20"),
    muted: readToken(host, "--c-muted", "#5c5e62"),
    accent: readToken(host, "--c-accent", "#3e6ae1"),
    border: readToken(host, "--c-border-i", "#d0d1d2"),
    font: font || PRINT_THEME.font,
  };
}

/** Mermaid's `base` theme, driven by the interface's tokens instead of its own. */
function themeVariables(theme: DiagramTheme): Record<string, string> {
  return {
    background: theme.surface,
    fontFamily: theme.font,
    fontSize: "14px",
    // Nodes: the tinted panel a code block sits on, so a diagram and a code
    // block read as the same kind of object.
    primaryColor: theme.panel,
    primaryTextColor: theme.text,
    primaryBorderColor: theme.border,
    secondaryColor: theme.surface,
    secondaryTextColor: theme.text,
    secondaryBorderColor: theme.border,
    tertiaryColor: theme.panel,
    tertiaryTextColor: theme.text,
    mainBkg: theme.panel,
    nodeBorder: theme.border,
    nodeTextColor: theme.text,
    clusterBkg: theme.surface,
    clusterBorder: theme.border,
    lineColor: theme.muted,
    textColor: theme.text,
    titleColor: theme.text,
    edgeLabelBackground: theme.surface,
    labelColor: theme.text,
    // Sequence diagrams.
    actorBkg: theme.panel,
    actorBorder: theme.border,
    actorTextColor: theme.text,
    signalColor: theme.muted,
    signalTextColor: theme.text,
    labelBoxBkgColor: theme.surface,
    labelBoxBorderColor: theme.border,
    labelTextColor: theme.text,
    loopTextColor: theme.text,
    noteBkgColor: theme.panel,
    noteBorderColor: theme.border,
    noteTextColor: theme.text,
    sequenceNumberColor: theme.surface,
    // Gantt / timeline.
    sectionBkgColor: theme.panel,
    altSectionBkgColor: theme.surface,
    sectionBkgColor2: theme.panel,
    taskBkgColor: theme.accent,
    taskTextColor: theme.surface,
    taskTextOutsideColor: theme.text,
    activeTaskBkgColor: theme.accent,
    activeTaskBorderColor: theme.accent,
    doneTaskBkgColor: theme.panel,
    doneTaskBorderColor: theme.border,
    gridColor: theme.border,
    todayLineColor: theme.accent,
    // Pie.
    pie1: theme.accent,
    pie2: theme.muted,
    pie3: theme.border,
    pie4: theme.panel,
    pieTitleTextColor: theme.text,
    pieSectionTextColor: theme.surface,
    pieLegendTextColor: theme.text,
    pieStrokeColor: theme.surface,
    pieOuterStrokeColor: theme.border,
  };
}

let appliedTheme = "";

async function configure(api: MermaidApi, theme: DiagramTheme): Promise<void> {
  const key = JSON.stringify(theme);
  if (key === appliedTheme) return;
  api.initialize({
    startOnLoad: false,
    // Model text, so: labels stay escaped text (`htmlLabels: false`) and
    // Mermaid sanitises rather than trusting the source.
    securityLevel: "strict",
    htmlLabels: false,
    // A draw that throws must not leave Mermaid's error diagram in a temp
    // `<div>` appended to `<body>` (its cleanup only runs in this mode), and
    // the card's own `showSource()` is a better fallback than that error box.
    suppressErrorRendering: true,
    theme: "base",
    themeVariables: themeVariables(theme),
    fontFamily: theme.font,
  });
  appliedTheme = key;
}

const FORBIDDEN_ELEMENTS = new Set([
  "script",
  "foreignobject",
  "iframe",
  "object",
  "embed",
  "audio",
  "video",
  "canvas",
  "image",
]);

const EXTERNAL_URL = /url\s*\(\s*['"]?(?!#)/i;

/**
 * Strip anything executable from a rendered diagram.
 *
 * `securityLevel: "strict"` already refuses most of this, but the SVG is
 * inserted with `innerHTML`, so the last word belongs here: scripts are
 * removed with their subtree, event handlers and external references are
 * dropped. Mermaid's `<style>` stays (it is scoped to the generated id) unless
 * it tries to `@import` or load something.
 */
function pruneMermaidSvg(markup: string): string {
  const doc = new DOMParser().parseFromString(markup, "text/html");
  const root = doc.querySelector("svg");
  if (!root) throw new Error("mermaid produced no svg");
  for (const element of Array.from(root.querySelectorAll("*"))) {
    if (FORBIDDEN_ELEMENTS.has(element.localName.toLowerCase())) {
      element.remove();
      continue;
    }
    for (const attr of Array.from(element.attributes)) {
      const name = attr.name.toLowerCase();
      if (name.startsWith("on")) {
        element.removeAttribute(attr.name);
        continue;
      }
      if ((name === "href" || name === "xlink:href") && !attr.value.trim().startsWith("#")) {
        element.removeAttribute(attr.name);
        continue;
      }
      if (EXTERNAL_URL.test(attr.value)) element.removeAttribute(attr.name);
    }
  }
  for (const style of Array.from(root.querySelectorAll("style"))) {
    if (/@import/i.test(style.textContent ?? "") || EXTERNAL_URL.test(style.textContent ?? "")) {
      style.remove();
    }
  }
  return new XMLSerializer().serializeToString(root);
}

let counter = 0;

async function renderSource(source: string, theme: DiagramTheme): Promise<string> {
  const api = await loadMermaid();
  await configure(api, theme);
  const id = `mermaid-slot-${(counter += 1)}`;
  // A stale container with this id (a render that threw) would be reused by
  // Mermaid and paint over itself.
  document.getElementById(id)?.remove();
  const parsed = await api.parse(source, { suppressErrors: true });
  if (!parsed) throw new Error("invalid mermaid source");
  const { svg } = await api.render(id, source);
  return pruneMermaidSvg(svg);
}

// ─── Cards ──────────────────────────────────────────────────────────────────

interface DiagramHandle {
  element: HTMLElement;
  /** Render (or re-render) into the card. Idempotent per theme. */
  paint: () => void;
}

/** Newest 48 diagrams stay alive; a longer conversation rebuilds one on scroll back. */
const MAX_CACHED = 48;

const cache = new Map<string, DiagramHandle>();

/** Card → Mermaid source, so the print/PDF export can re-render it. */
const sources = new WeakMap<HTMLElement, string>();

// A theme change bakes new colours into the SVG, so every live diagram re-renders.
const redraws = new Set<() => void>();

/**
 * A graph several times wider than the card would be scaled down to a
 * illegible strip. Past that point it keeps its natural width and the card
 * scrolls sideways instead of shrinking.
 */
function fitDiagram(svg: SVGSVGElement, viewport: HTMLElement): void {
  const viewBox = (svg.getAttribute("viewBox") ?? "").split(/[\s,]+/).map(Number);
  const natural = viewBox.length === 4 && Number.isFinite(viewBox[2]) ? viewBox[2] : 0;
  const figure = svg.closest(".chat-mermaid") as HTMLElement | null;
  if (!figure) return;
  // `clientWidth` includes the viewport's 12px padding on both sides.
  const room = viewport.clientWidth - 24;
  if (natural <= 0 || room <= 0 || natural <= room * 2.5) {
    delete figure.dataset.diagramFit;
    svg.style.removeProperty("width");
    svg.style.removeProperty("max-width");
    return;
  }
  figure.dataset.diagramFit = "scroll";
  svg.style.width = `${Math.round(natural)}px`;
  svg.style.maxWidth = "none";
}

function buildCard(spec: DiagramSpec, index: number): DiagramHandle {
  const figure = document.createElement("figure");
  // The frame, caption bar and state styles are the figure card's: a chart is a
  // chart whichever renderer drew it.
  figure.className = "chat-figure chat-mermaid";
  figure.dataset.figureKind = "mermaid";

  const viewport = document.createElement("div");
  viewport.className = "chat-figure-viewport";

  const bar = document.createElement("figcaption");
  bar.className = "chat-figure-bar";
  const title = document.createElement("span");
  title.className = "chat-figure-title";
  title.textContent = spec.title || text("figure.diagramTitle", { index: index + 1 });
  bar.append(title);

  figure.append(viewport, bar);
  figure.setAttribute("aria-label", title.textContent ?? "");
  sources.set(figure, spec.source);

  const handle: DiagramHandle = { element: figure, paint: () => {} };

  if (spec.pending) {
    figure.dataset.figureState = "pending";
    viewport.textContent = text("figure.drawing");
    return handle;
  }

  // A diagram that cannot be drawn says so in one line and keeps its source a
  // click away, exactly like a figure (see `showUnrenderable`).
  const showSource = (reason: string) => {
    console.warn(`[figures] mermaid diagram not drawn (${reason})`, {
      title: spec.title,
      source: spec.source.slice(0, 400),
    });
    showUnrenderable(figure, viewport, spec.source);
  };

  let busy = false;
  let queued = false;

  function paint(): void {
    if (busy) {
      queued = true;
      return;
    }
    busy = true;
    // Keep a finished diagram on screen while it re-renders: replacing the SVG
    // is what removes the flicker a theme switch would otherwise cause.
    if (!viewport.querySelector("svg")) {
      figure.dataset.figureState = "loading";
      viewport.textContent = text("figure.drawing");
    }
    void enqueue(() => renderSource(spec.source, readTheme(figure)))
      .then((markup) => {
        viewport.innerHTML = markup;
        const svg = viewport.querySelector("svg");
        if (!svg) {
          showSource("render produced no svg");
          return;
        }
        svg.classList.add("chat-mermaid-svg");
        svg.setAttribute("role", "img");
        svg.setAttribute("aria-label", figure.getAttribute("aria-label") ?? "");
        fitDiagram(svg, viewport);
        figure.dataset.figureState = "ready";
      })
      .catch((error: unknown) => showSource(String(error)))
      .finally(() => {
        busy = false;
        if (queued) {
          queued = false;
          paint();
        }
      });
  }

  function redraw(): void {
    // A card that left the document (the reply was re-rendered without it) has
    // no reason to keep listening for theme changes.
    if (!figure.isConnected) {
      redraws.delete(redraw);
      return;
    }
    paint();
  }

  handle.paint = paint;
  redraws.add(redraw);
  return handle;
}

let watching = false;

function ensureWatching(): void {
  if (watching) return;
  watching = true;
  watchTheme(() => {
    for (const redraw of redraws) {
      try {
        redraw();
      } catch {
        /* one broken diagram must not stop the others */
      }
    }
  });
}

function prune(messageId: string, live: Set<string>): void {
  for (const key of cache.keys()) {
    if (key.startsWith(`${messageId}|`) && !live.has(key)) cache.delete(key);
  }
  while (cache.size > MAX_CACHED) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
}

/**
 * Swap every diagram slot of `root` for its card.
 *
 * Called right after `innerHTML` is committed. The card is built synchronously
 * (so the placeholder holds its place instead of collapsing the paragraph) and
 * paints itself once Mermaid has rendered.
 */
export function mountDiagrams(
  root: HTMLElement,
  diagrams: DiagramSpec[],
  options: { messageId: string },
): void {
  if (diagrams.length === 0) return;
  const slots = expandSlots(root, SLOT_OPTIONS);
  if (slots.length === 0) return;
  ensureWatching();

  const live = new Set<string>();
  for (const slot of slots) {
    const index = Number.parseInt(slot.dataset.mermaidSlot ?? "", 10);
    const spec = diagrams[index];
    if (!spec) {
      slot.remove();
      continue;
    }
    const key = `${options.messageId}|${index}|${hashSource(
      `${spec.title}\u0000${spec.pending ? "1" : "0"}\u0000${spec.source}`,
    )}`;
    live.add(key);
    let handle = cache.get(key);
    const fresh = !handle;
    if (!handle) {
      handle = buildCard(spec, index);
      cache.set(key, handle);
    }
    // Moving the cached card (instead of rebuilding it) keeps the rendered SVG
    // across the rebuild every streamed token causes.
    slotHost(slot).replaceWith(handle.element);
    if (fresh) handle.paint();
  }
  prune(options.messageId, live);
}

/**
 * Re-render every diagram of a reply for the printed page.
 *
 * The export document is always light, while the SVG on screen carries whatever
 * palette is active — a dark-mode reply would otherwise print light text on a
 * dark block. `live` and `clone` are the transcript subtree and its clone; the
 * cards are matched by position, since the clone's elements are new nodes.
 */
export async function inlineDiagramsForPrint(live: HTMLElement, clone: HTMLElement): Promise<void> {
  const liveCards = Array.from(live.querySelectorAll<HTMLElement>(".chat-mermaid"));
  if (liveCards.length === 0) return;
  const cloneCards = Array.from(clone.querySelectorAll<HTMLElement>(".chat-mermaid"));
  await Promise.all(
    liveCards.map(async (card, index) => {
      const target = cloneCards[index];
      const source = sources.get(card);
      if (!target || !source || card.dataset.figureState !== "ready") return;
      const viewport = target.querySelector(".chat-figure-viewport");
      if (!viewport) return;
      try {
        viewport.innerHTML = await enqueue(() => renderSource(source, PRINT_THEME));
        viewport.querySelector("svg")?.classList.add("chat-mermaid-svg");
      } catch {
        /* keep whatever the clone already carries */
      }
    }),
  );
}
