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

import { openFiguresDir, saveFigure } from "../api";
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
import { figureSlug, SLOT_PATTERN, type FigureSpec } from "./figures";
import { sanitizeSvg } from "./svgSanitize";

export interface MountFiguresOptions {
  /** Stable identity of the reply, so a cached card is reused. */
  messageId: string;
}

interface FigureHandle {
  element: HTMLElement;
  /** Called after the card is in the document (a size is measurable then). */
  activate: () => void;
  /** Base64 PNG of what the card currently shows (PDF export, save). */
  png: () => Promise<string>;
  save: () => Promise<boolean>;
}

/** Newest 48 cards stay alive; a longer conversation rebuilds one on scroll back. */
const MAX_CACHED = 48;

const cache = new Map<string, FigureHandle>();

/** Element → handle, so "save all" can await the real save it triggers. */
const handles = new WeakMap<HTMLElement, FigureHandle>();

/** FNV-1a — cheap, and only used to tell one figure version from another. */
function hash(value: string): string {
  let result = 0x811c9dc5;
  for (let i = 0; i < value.length; i += 1) {
    result ^= value.charCodeAt(i);
    result = Math.imul(result, 0x01000193);
  }
  return (result >>> 0).toString(36);
}

function readPalette(element: Element): CarvePalette {
  const computed = window.getComputedStyle(element);
  const read = (name: string, fallback: string) =>
    computed.getPropertyValue(name).trim() || fallback;
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
  };
}

// ─── Theme following ────────────────────────────────────────────────────────
//
// A 3D figure bakes its colours into pixels, so a theme switch has to redraw
// it. One observer for the whole app; cards that left the document drop out.

const themeRedraws = new Set<() => void>();
let themeObserver: MutationObserver | null = null;

function watchTheme(redraw: () => void): void {
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

// ─── Slot expansion and mounting ────────────────────────────────────────────

/** Replace every slot token in `root` with an empty `<span>`. */
function expandSlots(root: HTMLElement): HTMLSpanElement[] {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const texts: Text[] = [];
  while (walker.nextNode()) {
    const node = walker.currentNode as Text;
    if (node.data.includes("\u2063F")) texts.push(node);
  }
  const slots: HTMLSpanElement[] = [];
  for (const node of texts) {
    const fragment = document.createDocumentFragment();
    for (const part of node.data.split(/(\u2063F\d+\u2063)/g)) {
      SLOT_PATTERN.lastIndex = 0;
      const match = SLOT_PATTERN.exec(part);
      if (match && part === match[0]) {
        const slot = document.createElement("span");
        slot.className = "chat-figure-slot";
        slot.dataset.figureSlot = match[1];
        fragment.appendChild(slot);
        slots.push(slot);
      } else if (part) {
        fragment.appendChild(document.createTextNode(part));
      }
    }
    node.parentNode?.replaceChild(fragment, node);
  }
  return slots;
}

/** A figure alone in its paragraph is a block; next to prose it stays inline. */
function slotHost(slot: HTMLSpanElement): HTMLElement {
  const parent = slot.parentElement;
  if (parent && parent.tagName === "P") {
    const alone = Array.from(parent.childNodes).every(
      (node) => node === slot || (node.nodeType === Node.TEXT_NODE && !(node as Text).data.trim()),
    );
    if (alone) return parent;
  }
  return slot;
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
  const slots = expandSlots(root);
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
    const key = `${options.messageId}|${index}|${hash(
      `${spec.kind}\u0000${spec.title}\u0000${spec.pending ? "1" : "0"}\u0000${spec.source}`,
    )}`;
    live.add(key);
    let handle = cache.get(key);
    if (!handle) {
      handle = spec.kind === "carve" ? buildCarveCard(spec, index) : buildSvgCard(spec, index);
      cache.set(key, handle);
    }
    handles.set(handle.element, handle);
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
