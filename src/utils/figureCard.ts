/**
 * Mounts the figures of one reply into the transcript.
 *
 * `ChatMessage` writes the rendered markdown with `innerHTML`, so a figure
 * cannot be a React node; what it leaves behind is an invisible slot token
 * (see `figures.ts`). This module swaps each token for a real `<figure>` card —
 * a sanitised `<svg>` for 2D, a three.js canvas for 3D, a caption with the save
 * action — and caches it, because the whole subtree is replaced on every
 * streamed token. Rebuilding a three.js scene (or losing the orbit angle a user
 * just dragged) on each token would make the card flicker and fight the
 * pointer, so the cached element is *moved* back into the new tree instead:
 * the canvas keeps its pixels and the listeners stay attached.
 */

import { openFiguresDir, saveFigure, saveFigureHtml } from "../api";
import { t as text } from "../i18n";
import {
  defaultView,
  parseCarve,
  renderCarve,
  type CarvePalette,
  type CarveScene,
  type CarveView,
} from "./carve3d";
import { canvasToPngBase64, svgToPngBase64 } from "./figurePng";
import { figureSlug, type FigureSpec } from "./figures";
import {
  FRAME_MESSAGE_TAG,
  MAX_FRAME_HEIGHT,
  MAX_HTML_SOURCE,
  MIN_FRAME_HEIGHT,
  buildFrameDocument,
  buildStandaloneDocument,
  hasInlineScript,
  postFigureTokens,
  readFigureTokens,
} from "./htmlFigure";
import { parseFlow, renderFlowSvg, resolveFlowPalette } from "./flowDiagram";
import { expandSlots, hashSource, slotHost } from "./slotTokens";
import { sanitizeSvg } from "./svgSanitize";

export interface MountFiguresOptions {
  /** Stable identity of the reply, so a cached card is reused. */
  messageId: string;
  /**
   * Is this reply still streaming? A frame is a whole document, so the answer
   * decides *when* a dynamic figure may run — see `buildHtmlCard`.
   */
  streaming?: boolean;
}

/**
 * Session consent for running a fragment's script without a click.
 *
 * `sessionStorage` on purpose: it survives a reload but not a restart, so the
 * answer to "may this app run model-authored script while I read" is asked
 * again every time the app starts — the same question, asked once per session
 * instead of once per figure. The frame cannot read or write it (opaque
 * origin), and a figure that is refused never touches it.
 */
const AUTOPLAY_KEY = "figures.autoplayScripts";

function autoplayConsented(): boolean {
  try {
    return window.sessionStorage.getItem(AUTOPLAY_KEY) === "1";
  } catch {
    return false;
  }
}

function rememberAutoplayConsent(): void {
  try {
    window.sessionStorage.setItem(AUTOPLAY_KEY, "1");
  } catch {
    /* private mode: consent is then asked again on every figure */
  }
}

interface FigureHandle {
  element: HTMLElement;
  /** Called after the card is in the document (a size is measurable then). */
  activate: () => void;
  /** Base64 PNG of what the card currently shows (PDF export, save). */
  png: () => Promise<string>;
  save: () => Promise<boolean>;
  /** Release listeners when the card leaves the cache (a live frame has some). */
  dispose?: () => void;
  /**
   * Called just before a cached card is re-inserted into the tree. A frame
   * reloads when the element moves (that is how both engines behave), so any
   * counter that must not survive the move resets here — see `buildHtmlCard`.
   */
  beforeMount?: () => void;
}

/** Newest 48 cards stay alive; a longer conversation rebuilds one on scroll back. */
const MAX_CACHED = 48;

const cache = new Map<string, FigureHandle>();

/** Element → handle, so "save all" can await the real save it triggers. */
const handles = new WeakMap<HTMLElement, FigureHandle>();

/** Slot marker for a figure card; `figures.ts` emits it, this module mounts it. */
const SLOT_OPTIONS = { marker: "\u2063F", className: "chat-figure-slot", datasetKey: "figureSlot" };

/** One interface token, read live — a drawing that bakes colours follows the theme. */
export function readToken(element: Element, name: string, fallback: string): string {
  const computed = window.getComputedStyle(element);
  return computed.getPropertyValue(name).trim() || fallback;
}

function readPalette(element: Element): CarvePalette {
  const read = (name: string, fallback: string) => readToken(element, name, fallback);
  return {
    // The viewport's own fill: a saved PNG and a 3D canvas both continue the
    // sheet the drawing sits on instead of introducing a third surface.
    background: read("--c-surface", "#ffffff"),
    accent: read("--c-accent", "#3e6ae1"),
    success: read("--c-success", "#1f8a3b"),
    warning: read("--c-warning", "#b07500"),
    danger: read("--c-danger", "#c0392b"),
    text: read("--c-text", "#171a20"),
    muted: read("--c-muted", "#5c5e62"),
    // Label sprites are drawn in the interface's typeface.
    font: window.getComputedStyle(element).fontFamily || "system-ui, sans-serif",
  };
}

// ─── Theme following ────────────────────────────────────────────────────────
//
// A 3D figure bakes its colours into pixels, so a theme switch has to redraw
// it. One observer for the whole app; cards that left the document drop out.

const themeRedraws = new Set<() => void>();
let themeObserver: MutationObserver | null = null;

export function watchTheme(redraw: () => void): void {
  themeRedraws.add(redraw);
  if (themeObserver) return;
  themeObserver = new MutationObserver(() => {
    for (const callback of themeRedraws) {
      try {
        callback();
      } catch {
        /* one broken figure must not stop the others */
      }
    }
  });
  themeObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["data-theme", "data-palette", "data-glass"],
  });
}

// ─── Card construction ──────────────────────────────────────────────────────

interface CardParts {
  figure: HTMLElement;
  viewport: HTMLElement;
  actions: HTMLElement;
  status: HTMLElement;
}

function buildCard(spec: FigureSpec, index: number): CardParts {
  const figure = document.createElement("figure");
  figure.className = "chat-figure";
  figure.dataset.figureKind = spec.kind;

  const viewport = document.createElement("div");
  viewport.className = "chat-figure-viewport";

  const bar = document.createElement("figcaption");
  bar.className = "chat-figure-bar";

  const title = document.createElement("span");
  title.className = "chat-figure-title";
  title.textContent = spec.title || text("figure.title", { index: index + 1 });

  const status = document.createElement("span");
  status.className = "chat-figure-status";
  status.setAttribute("role", "status");
  status.title = text("figure.openFolder");
  // Once a figure is saved, its status line is the way to the folder.
  status.addEventListener("click", () => {
    if (status.dataset.state === "ok") void openFiguresDir();
  });

  const actions = document.createElement("span");
  actions.className = "chat-figure-actions";

  bar.append(title, status, actions);
  figure.append(viewport, bar);
  figure.setAttribute("aria-label", title.textContent ?? "");
  return { figure, viewport, actions, status };
}

function addButton(
  parent: HTMLElement,
  label: string,
  className: string,
  onClick: () => void,
): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = `chat-figure-btn ${className}`;
  button.textContent = label;
  button.addEventListener("click", (event) => {
    event.preventDefault();
    event.stopPropagation();
    onClick();
  });
  parent.appendChild(button);
  return button;
}

/** The source as a code block: an unrenderable figure still shows its markup. */
function showSource(parts: CardParts, source: string): void {
  parts.figure.dataset.figureState = "invalid";
  const pre = document.createElement("pre");
  pre.className = "hljs chat-figure-source";
  const code = document.createElement("code");
  code.textContent = source;
  pre.appendChild(code);
  parts.viewport.replaceChildren(pre);
}

/** Where a saved file lands, plus the failure message. Shared by both kinds. */
function saveStatus(
  parts: CardParts,
  button: HTMLButtonElement,
  run: () => Promise<string>,
): Promise<boolean> {
  if (button.disabled) return Promise.resolve(false);
  button.disabled = true;
  const idle = button.textContent ?? "";
  button.textContent = text("figure.saving");
  parts.status.dataset.state = "";
  parts.status.textContent = "";
  return run()
    .then((path) => {
      parts.status.textContent = text("figure.saved", { path });
      parts.status.dataset.state = "ok";
      return true;
    })
    .catch(() => {
      parts.status.textContent = text("figure.saveFailed");
      parts.status.dataset.state = "error";
      return false;
    })
    .finally(() => {
      button.disabled = false;
      button.textContent = idle;
    });
}

const noop = () => {};

const unrenderable = (parts: CardParts): FigureHandle => ({
  element: parts.figure,
  activate: noop,
  png: async () => {
    throw new Error("figure is not renderable");
  },
  save: async () => false,
});

function buildSvgCard(spec: FigureSpec, index: number): FigureHandle {
  const parts = buildCard(spec, index);

  if (spec.pending) {
    parts.figure.dataset.figureState = "pending";
    parts.viewport.textContent = text("figure.drawing");
    return unrenderable(parts);
  }

  const markup = sanitizeSvg(spec.source);
  if (markup) parts.viewport.innerHTML = markup;
  const svg = parts.viewport.querySelector<SVGSVGElement>("svg");
  if (!svg) {
    showSource(parts, spec.source);
    return unrenderable(parts);
  }

  svg.classList.add("chat-figure-svg");
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", parts.figure.getAttribute("aria-label") ?? "");
  // Fill the card width; the viewBox keeps the drawing's aspect ratio.
  svg.removeAttribute("height");
  svg.setAttribute("width", "100%");
  parts.figure.dataset.figureState = "ready";

  const png = (): Promise<string> =>
    svgToPngBase64(svg, { background: readPalette(parts.figure).background });

  const button = addButton(parts.actions, text("figure.save"), "chat-figure-save", () => {
    void save();
  });

  const save = (): Promise<boolean> =>
    saveStatus(parts, button, async () => {
      const saved = await saveFigure({ id: figureSlug(spec, index), pngBase64: await png() });
      return saved.path;
    });

  return { element: parts.figure, activate: noop, png, save };
}

function buildCarveCard(spec: FigureSpec, index: number): FigureHandle {
  const parts = buildCard(spec, index);

  if (spec.pending) {
    parts.figure.dataset.figureState = "pending";
    parts.viewport.textContent = text("figure.drawing");
    return unrenderable(parts);
  }

  const scene: CarveScene | null = parseCarve(spec.source);
  if (!scene) {
    showSource(parts, spec.source);
    return unrenderable(parts);
  }

  parts.figure.dataset.figureState = "loading";
  const canvas = document.createElement("canvas");
  canvas.className = "chat-figure-canvas";
  // Fixed 16:10 backing store until the first frame: the card must not resize
  // while a scene is still loading three.js.
  canvas.width = 640;
  canvas.height = 400;
  parts.viewport.replaceChildren(canvas);

  const home = defaultView(scene);
  const view: CarveView = { ...home };
  let ready = false;
  let drawnWidth = 0;
  let busy = false;
  let queued = false;

  const draw = async (): Promise<void> => {
    if (busy) {
      queued = true;
      return;
    }
    busy = true;
    try {
      const width = Math.round(canvas.clientWidth || 640);
      canvas.width = Math.max(2, width);
      canvas.height = Math.max(2, Math.round(width * 0.625));
      const ok = await renderCarve(canvas, scene, view, readPalette(parts.figure));
      ready = ok;
      drawnWidth = width;
      parts.figure.dataset.figureState = ok ? "ready" : "error";
      if (!ok) parts.viewport.title = text("figure.webglUnavailable");
    } finally {
      busy = false;
      if (queued) {
        queued = false;
        void draw();
      }
    }
  };

  const redraw = () => {
    if (!parts.figure.isConnected) {
      themeRedraws.delete(redraw);
      return;
    }
    void draw();
  };

  // Orbit by dragging. No wheel zoom on purpose: the transcript scrolls with
  // the wheel, and swallowing it inside a reply is worse than a missing zoom.
  canvas.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    const startX = event.clientX;
    const startY = event.clientY;
    const startYaw = view.yaw;
    const startPitch = view.pitch;
    canvas.setPointerCapture(event.pointerId);
    const move = (moveEvent: PointerEvent) => {
      view.yaw = startYaw - (moveEvent.clientX - startX) * 0.45;
      view.pitch = Math.max(-89, Math.min(89, startPitch + (moveEvent.clientY - startY) * 0.35));
      void draw();
    };
    const end = () => {
      canvas.removeEventListener("pointermove", move);
      canvas.removeEventListener("pointerup", end);
      canvas.removeEventListener("pointercancel", end);
    };
    canvas.addEventListener("pointermove", move);
    canvas.addEventListener("pointerup", end);
    canvas.addEventListener("pointercancel", end);
  });

  addButton(parts.actions, text("figure.resetView"), "chat-figure-reset", () => {
    view.yaw = home.yaw;
    view.pitch = home.pitch;
    view.zoom = home.zoom;
    redraw();
  });

  const button = addButton(parts.actions, text("figure.save"), "chat-figure-save", () => {
    void save();
  });

  const png = async (): Promise<string> => {
    if (!ready) throw new Error("figure is still rendering");
    return canvasToPngBase64(canvas);
  };

  const save = (): Promise<boolean> =>
    saveStatus(parts, button, async () => {
      const saved = await saveFigure({ id: figureSlug(spec, index), pngBase64: await png() });
      return saved.path;
    });

  watchTheme(redraw);

  return {
    element: parts.figure,
    activate: () => {
      if (!ready || Math.abs(canvas.clientWidth - drawnWidth) > 8) void draw();
    },
    png,
    save,
  };
}

/**
 * A flow figure: the declarative flowchart (spec and renderer in
 * `flowDiagram.ts`).
 *
 * The card work is the svg card's — a themed, rasterisable inline SVG with a
 * save button — plus two differences: the render is asynchronous (dagre is a
 * lazy chunk, imported on the first flow figure), and a graph wider than the
 * card scrolls sideways instead of shrinking to an illegible strip (the
 * mermaid behaviour). Colours are baked into the SVG, so a theme switch
 * re-renders rather than re-styles.
 */
function buildFlowCard(spec: FigureSpec, index: number): FigureHandle {
  const parts = buildCard(spec, index);

  if (spec.pending) {
    parts.figure.dataset.figureState = "pending";
    parts.viewport.textContent = text("figure.drawing");
    return unrenderable(parts);
  }

  const parsed = parseFlow(spec.source);
  if (!parsed) {
    showSource(parts, spec.source);
    return unrenderable(parts);
  }

  const caption = spec.title || text("figure.title", { index: index + 1 });

  const button = addButton(parts.actions, text("figure.save"), "chat-figure-save", () => {
    void save();
  });

  const png = async (): Promise<string> => {
    const svg = parts.viewport.querySelector<SVGSVGElement>("svg");
    if (!svg) throw new Error("figure is not drawn yet");
    return svgToPngBase64(svg, { background: readPalette(parts.figure).background });
  };

  const save = (): Promise<boolean> =>
    saveStatus(parts, button, async () => {
      const saved = await saveFigure({
        id: figureSlug(spec, index),
        pngBase64: await png(),
      });
      return saved.path;
    });

  let busy = false;
  let queued = false;

  const paint = (): void => {
    if (busy) {
      queued = true;
      return;
    }
    busy = true;
    void renderFlowSvg(parsed, resolveFlowPalette(parts.figure), caption)
      .then((svg) => {
        // A spec that parsed but did not lay out shows its source, like a
        // scene that parsed but drew nothing.
        if (!svg) {
          showSource(parts, spec.source);
          return;
        }
        svg.classList.add("chat-figure-svg");
        parts.viewport.replaceChildren(svg);
        fitFlowDiagram(svg, parts.viewport, parts.figure);
        parts.figure.dataset.figureState = "ready";
      })
      .catch(() => showSource(parts, spec.source))
      .finally(() => {
        busy = false;
        if (queued) {
          queued = false;
          paint();
        }
      });
  };

  const redraw = () => {
    // A card that left the document stops listening for theme changes.
    if (!parts.figure.isConnected) {
      themeRedraws.delete(redraw);
      return;
    }
    paint();
  };

  watchTheme(redraw);
  paint();

  return { element: parts.figure, activate: noop, png, save };
}

/**
 * Keep a flow graph at natural width past ~2.5× the card (it would otherwise
 * scale to an illegible strip) and let the card scroll sideways — the rule the
 * mermaid card applies. Its copy lives in `mermaidDiagrams.ts` because
 * importing it here would be a cycle: it imports this module's theme watcher.
 */
function fitFlowDiagram(svg: SVGSVGElement, viewport: HTMLElement, figure: HTMLElement): void {
  const viewBox = (svg.getAttribute("viewBox") ?? "").split(/[\s,]+/).map(Number);
  const natural = viewBox.length === 4 && Number.isFinite(viewBox[2]) ? viewBox[2] : 0;
  // `clientWidth` includes the viewport's 12px padding on both sides.
  const room = viewport.clientWidth - 24;
  if (natural <= 0 || room <= 0 || natural <= room * 2.5) {
    delete figure.dataset.diagramFit;
    svg.style.removeProperty("width");
    // Never upscale: a three-node chart blown up 3× to the card width looks
    // like clip art. `max-width` at natural size keeps small charts tidy and
    // still lets a wide one shrink to fit (the mermaid rule).
    if (natural > 0) svg.style.maxWidth = `${Math.round(natural)}px`;
    return;
  }
  figure.dataset.diagramFit = "scroll";
  svg.style.width = `${Math.round(natural)}px`;
  svg.style.maxWidth = "none";
}

/**
 * A dynamic figure: one sandboxed frame holding the model's HTML5 fragment.
 *
 * The frame is the boundary (see `htmlFigure.ts`), so nothing here sanitises
 * the markup — it runs in an opaque origin instead. Two things cross back: the
 * height the wrapper reports, and nothing else. `event.source` is checked on
 * every message, so one card's reporter cannot resize another reply's drawing.
 *
 * There is no PNG: an animation has no single frame, and a cross-origin frame
 * cannot be rasterised from here anyway. The save action writes the fragment as
 * a standalone `.html`, tokens baked — which is what a dynamic figure *is*.
 *
 * A fragment that cannot run script starts on mount — markup, CSS and SVG
 * animation cannot block the thread they share with the transcript. A fragment
 * that can needs one answer first, because on WebKitGTK *and* Chromium a
 * sandboxed frame shares the web process with the transcript: one
 * `while (true) {}` freezes the whole window, and no watchdog can end it because
 * the watchdog runs on the frozen thread (measured in both: a 2 s loop leaves a
 * 2 s hole in the parent's 50 ms timer). So the tiers are: CSS runs, script
 * waits for a click — or for the session consent a click records (`AUTOPLAY_KEY`
 * above). The gate also keeps the one hole CSP cannot close (`loads` below)
 * behind the same click.
 */
function buildHtmlCard(
  spec: FigureSpec,
  index: number,
  options: MountFiguresOptions,
): FigureHandle {
  const parts = buildCard(spec, index);

  if (spec.pending) {
    parts.figure.dataset.figureState = "pending";
    parts.viewport.textContent = text("figure.drawing");
    return unrenderable(parts);
  }

  if (!spec.source.trim() || spec.source.length > MAX_HTML_SOURCE) {
    // Too large is not "broken": the source is intact and worth showing, and the
    // status line says why it did not run.
    showSource(parts, spec.source);
    parts.status.dataset.state = "error";
    parts.status.textContent = text("figure.htmlTooLarge");
    return unrenderable(parts);
  }

  const tokens = () => readFigureTokens(parts.figure);
  const caption = spec.title || text("figure.title", { index: index + 1 });

  const button = addButton(parts.actions, text("figure.saveHtml"), "chat-figure-save", () => {
    void save();
  });

  const save = (): Promise<boolean> =>
    saveStatus(parts, button, async () => {
      const saved = await saveFigureHtml({
        id: figureSlug(spec, index),
        html: buildStandaloneDocument(spec.source, tokens(), caption),
      });
      return saved.path;
    });

  let frame: HTMLIFrameElement | null = null;
  let stopMessages: (() => void) | null = null;
  // Load count of the live frame. Reset when the frame document starts and when
  // the card is re-inserted (a move reloads the document without the fragment
  // navigating), so exactly one load is the document this card wrote.
  let loadCount = 0;

  // ── The gate ──
  const poster = document.createElement("div");
  poster.className = "chat-figure-poster";

  const glyph = document.createElement("span");
  glyph.className = "chat-figure-poster-glyph";
  glyph.setAttribute("aria-hidden", "true");
  glyph.textContent = "▶";

  const label = document.createElement("span");
  label.className = "chat-figure-poster-label";

  const hint = document.createElement("span");
  hint.className = "chat-figure-poster-hint";

  const playButton = document.createElement("button");
  playButton.type = "button";
  playButton.className = "chat-figure-poster-play";
  playButton.append(glyph, label);

  const rememberButton = document.createElement("button");
  rememberButton.type = "button";
  rememberButton.className = "chat-figure-poster-remember";

  poster.append(playButton, hint, rememberButton);

  const showPoster = (): void => {
    // Mid-stream the frame would be reloaded by every commit (a commit replaces
    // the subtree, and a re-inserted frame starts over), so the card holds the
    // figure until the last commit; only then does the click become the gate.
    const waiting = Boolean(options.streaming);
    poster.dataset.mode = waiting ? "waiting" : "ready";
    playButton.disabled = waiting;
    rememberButton.disabled = waiting;
    label.textContent = text(options.streaming ? "figure.playsWhenDone" : "figure.playDynamic");
    hint.textContent = text("figure.sandboxHint");
    rememberButton.textContent = text("figure.rememberSession");
    parts.figure.dataset.figureState = options.streaming ? "deferred" : "gate";
    parts.viewport.replaceChildren(poster);
  };

  /** Drop the frame and say why: the figure left the document it was given. */
  const closeFrame = (notice: string): void => {
    stopMessages?.();
    stopMessages = null;
    frame = null;
    parts.figure.dataset.figureState = "blocked";
    const line = document.createElement("p");
    line.className = "chat-figure-notice";
    line.textContent = notice;
    parts.viewport.replaceChildren(line);
    parts.status.dataset.state = "error";
    parts.status.textContent = notice;
  };

  const play = (): void => {
    if (frame || options.streaming) return;
    parts.figure.dataset.figureState = "loading";

    const element = document.createElement("iframe");
    element.className = "chat-figure-frame";
    // `allow-scripts` and *only* that: without `allow-same-origin` the frame gets
    // an opaque origin, so it cannot read the transcript, the app's storage or its
    // cookies, and it cannot navigate the top window.
    element.setAttribute("sandbox", "allow-scripts");
    element.setAttribute("referrerpolicy", "no-referrer");
    element.setAttribute("title", parts.figure.getAttribute("aria-label") ?? "");
    // A height before the first report, so the card does not jump when the
    // fragment arrives with a different idea of its own size.
    element.style.height = `${MIN_FRAME_HEIGHT + 80}px`;
    frame = element;

    const onMessage = (event: MessageEvent) => {
      if (event.source !== element.contentWindow) return;
      const data = event.data as { htmlFig?: number; height?: number } | null;
      if (!data || data.htmlFig !== FRAME_MESSAGE_TAG || typeof data.height !== "number") return;
      if (!Number.isFinite(data.height)) return;
      const height = Math.min(
        MAX_FRAME_HEIGHT,
        Math.max(MIN_FRAME_HEIGHT, Math.round(data.height)),
      );
      element.style.height = `${height}px`;
    };
    window.addEventListener("message", onMessage);
    stopMessages = () => window.removeEventListener("message", onMessage);

    // `srcdoc` is set *before* insertion, and before the load counter starts: an
    // empty frame inserted first commits `about:blank` and fires that commit's
    // load, which the navigation guard below would read as the fragment leaving
    // the document one step early.
    element.srcdoc = buildFrameDocument(spec.source, tokens());
    loadCount = 0;

    // `default-src 'none'` refuses every request the fragment makes, but it does
    // not govern navigation — and a navigation *is* a request, with the URL as
    // the payload. The sandbox still applies to the document that arrives, so the
    // figure cannot escape; the point of counting loads is to stop the request
    // and to not draw an outside page inside a reply. One load is the document
    // this card wrote, a second one is the fragment leaving it.
    element.addEventListener("load", () => {
      loadCount += 1;
      if (loadCount === 1) {
        parts.figure.dataset.figureState = "ready";
        return;
      }
      closeFrame(text("figure.navigatedAway"));
    });

    parts.viewport.replaceChildren(element);
  };

  poster.addEventListener("click", play);

  rememberButton.addEventListener("click", () => {
    rememberAutoplayConsent();
    play();
  });

  // A theme switch re-sends the tokens instead of rewriting `srcdoc`: the
  // animation keeps its place, which a rebuild would throw away.
  const redraw = () => {
    if (!parts.figure.isConnected) {
      themeRedraws.delete(redraw);
      return;
    }
    if (frame) postFigureTokens(frame, tokens());
  };
  watchTheme(redraw);

  // Tiered start: markup/CSS animation cannot hang the thread it shares with
  // the transcript, so it runs on the settle commit; script waits for one
  // answer — a click, or the session consent a click recorded. Mid-stream even
  // a safe frame waits, because every commit would reload it.
  const gated = hasInlineScript(spec.source) && !autoplayConsented();
  if (!options.streaming && !gated) play();
  else showPoster();

  return {
    element: parts.figure,
    activate: noop,
    png: async () => {
      throw new Error("a dynamic figure has no single frame");
    },
    save,
    dispose: () => {
      stopMessages?.();
      themeRedraws.delete(redraw);
    },
    beforeMount: () => {
      loadCount = 0;
      // Consent may have arrived after this card was built — the remember click
      // of another figure. A gate that is no longer needed lifts at the next
      // rebuild of this reply; a card the guard blocked stays blocked.
      if (parts.figure.dataset.figureState === "gate" && !options.streaming && autoplayConsented()) {
        play();
      }
    },
  };
}

// ─── Slot expansion and mounting ────────────────────────────────────────────

function prune(messageId: string, live: Set<string>): void {
  for (const key of cache.keys()) {
    if (key.startsWith(`${messageId}|`) && !live.has(key)) {
      cache.get(key)?.dispose?.();
      cache.delete(key);
    }
  }
  while (cache.size > MAX_CACHED) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.get(oldest)?.dispose?.();
    cache.delete(oldest);
  }
}

/**
 * Swap every figure slot of `root` for its card.
 *
 * Called right after `innerHTML` is committed, before paint.
 */
export function mountFigures(
  root: HTMLElement,
  figures: FigureSpec[],
  options: MountFiguresOptions,
): void {
  if (figures.length === 0) return;
  const slots = expandSlots(root, SLOT_OPTIONS);
  if (slots.length === 0) return;

  const live = new Set<string>();
  for (const slot of slots) {
    const index = Number.parseInt(slot.dataset.figureSlot ?? "", 10);
    const spec = figures[index];
    if (!spec) {
      slot.remove();
      continue;
    }
    // The source is part of the key: an unfinished figure is a new version on
    // every token and must be rebuilt, while a finished one keeps its canvas,
    // its orbit angle and its "saved" line across rebuilds.
    // The streaming flag is part of it too: a dynamic figure mounts differently
    // before and after the reply settles (`buildHtmlCard`), so the settle commit
    // must build a card rather than reuse the deferred one.
    const key = `${options.messageId}|${index}|${options.streaming ? "s" : "c"}|${hashSource(
      `${spec.kind}\u0000${spec.title}\u0000${spec.pending ? "1" : "0"}\u0000${spec.source}`,
    )}`;
    live.add(key);
    let handle = cache.get(key);
    if (!handle) {
      if (spec.kind === "carve") handle = buildCarveCard(spec, index);
      else if (spec.kind === "html-fig") handle = buildHtmlCard(spec, index, options);
      else if (spec.kind === "flow") handle = buildFlowCard(spec, index);
      else handle = buildSvgCard(spec, index);
      cache.set(key, handle);
    }
    handles.set(handle.element, handle);
    // A re-inserted iframe reloads its document (both engines), so a card that
    // carries a live frame gets its load counter back to zero first: the reload
    // is not the fragment navigating away, it is the frame starting over.
    handle.beforeMount?.();
    slotHost(slot).replaceWith(handle.element);
    handle.activate();
  }
  prune(options.messageId, live);
}

/**
 * Save every figure of a reply, in reading order.
 *
 * Each card's save button is the single implementation of a save, so this
 * walks the mounted cards instead of duplicating the rasterisation path.
 */
export async function saveAllFigures(root: HTMLElement): Promise<number> {
  let saved = 0;
  for (const card of Array.from(root.querySelectorAll<HTMLElement>(".chat-figure"))) {
    const handle = handles.get(card);
    if (!handle) continue;
    if (await handle.save()) saved += 1;
  }
  return saved;
}
