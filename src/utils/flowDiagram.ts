/**
 * `flow` — declarative flowcharts: JSON in, a designed SVG out.
 *
 * A flowchart is the drawing a chat makes most, and it is also the one a
 * language model draws worst by hand: an `svg` block asks the model to solve
 * coordinate geometry (box positions, arrow routing, text overflow), and the
 * transcript renders the mistakes. Mermaid gets the layout right but its look
 * is its own, and it cannot animate.
 *
 * So a flowchart gets the `<carve>` treatment: the model declares *what* —
 * nodes with labels, edges between them — and this module decides *how*. Node
 * sizes come from measured text, positions from dagre's layered layout, and
 * every colour from the interface's tokens, so a flow figure is consistent by
 * construction, follows every theme and palette, and is rasterised like any
 * other drawn figure.
 *
 * Motion is CSS/SMIL, never script (`anim: "flow"` — dashes and a packet
 * travelling each edge; `anim: "step"` — nodes light up in rank order). That
 * keeps a dynamic flowchart in the *safe* tier of `figureCard.ts`: it plays on
 * the settle commit without the script-consent gate, and it still saves as a
 * PNG. `prefers-reduced-motion` skips the animation at build time *and* inside
 * the emitted style, either of which alone is enough.
 *
 * dagre is imported lazily (a ~15 KB gzipped chunk) so a conversation without
 * a flow figure never downloads it — the same shape as mermaid and three.js.
 */

import type { Graph as DagreGraph } from "@dagrejs/dagre";

// ─── Spec ────────────────────────────────────────────────────────────────────

/** The node vocabulary. A name outside this list renders as `process`. */
export type FlowShape = "process" | "start" | "end" | "decision" | "io" | "database";

/** Colour emphasis of a node, resolved to interface tokens. */
export type FlowTone = "default" | "accent" | "success" | "warning" | "danger";

export interface FlowNode {
  id: string;
  label?: string;
  shape?: FlowShape;
  tone?: FlowTone;
}

export interface FlowEdge {
  from: string;
  to: string;
  label?: string;
}

/** How the figure moves. `step` lights nodes up in layout order. */
export interface FlowAnim {
  mode?: "none" | "flow" | "step";
  /** 0.5–3; scales every duration. Default 1. */
  speed?: number;
}

export interface FlowSpec {
  /** Layer direction. Default `LR` — reading order for a chat column. */
  direction?: "LR" | "TB";
  anim?: FlowAnim;
  nodes: FlowNode[];
  edges?: FlowEdge[];
  /** Fence-line caption lands here via `parseFenceMeta` when absent. */
  title?: string;
  name?: string;
}

/** A spec past these sizes is a poster, not a diagram: show the source. */
const MAX_NODES = 48;
const MAX_EDGES = 96;
const MAX_LABEL = 60;

const SHAPES: readonly FlowShape[] = ["process", "start", "end", "decision", "io", "database"];
const TONES: readonly FlowTone[] = ["default", "accent", "success", "warning", "danger"];
const DIRECTIONS: readonly ("LR" | "TB")[] = ["LR", "TB"];

function str(value: unknown, max: number): string {
  return typeof value === "string" ? value.trim().slice(0, max) : "";
}

function pick<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return typeof value === "string" && (allowed as readonly string[]).includes(value)
    ? (value as T)
    : fallback;
}

/** Parse a `flow` body; `null` when it is not a flowchart at all. */
export function parseFlow(source: string): FlowSpec | null {
  let raw: unknown;
  try {
    raw = JSON.parse(source);
  } catch {
    return null;
  }
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const rawSpec = raw as Record<string, unknown>;

  const rawNodes = Array.isArray(rawSpec.nodes) ? rawSpec.nodes : [];
  if (rawNodes.length === 0 || rawNodes.length > MAX_NODES) return null;
  const ids = new Set<string>();
  const nodes: FlowNode[] = [];
  for (const entry of rawNodes) {
    if (!entry || typeof entry !== "object") return null;
    const node = entry as Record<string, unknown>;
    const id = str(node.id, 64);
    // An id is the edge endpoint: duplicate or missing means the edges could
    // attach to the wrong box, so the whole spec is rejected, not repaired.
    if (!id || ids.has(id)) return null;
    ids.add(id);
    nodes.push({
      id,
      label: str(node.label, MAX_LABEL) || id,
      shape: pick(node.shape, SHAPES, "process"),
      tone: pick(node.tone, TONES, "default"),
    });
  }

  const rawEdges = Array.isArray(rawSpec.edges) ? rawSpec.edges : [];
  if (rawEdges.length > MAX_EDGES) return null;
  const edges: FlowEdge[] = [];
  for (const entry of rawEdges) {
    if (!entry || typeof entry !== "object") return null;
    const edge = entry as Record<string, unknown>;
    const from = str(edge.from, 64);
    const to = str(edge.to, 64);
    // Endpoints must exist, and a self-loop has no good layout in dagre — it
    // is dropped rather than drawn as a knot.
    if (!from || !to || !ids.has(from) || !ids.has(to) || from === to) continue;
    edges.push({ from, to, label: str(edge.label, MAX_LABEL) });
  }

  let animMode: "none" | "flow" | "step" = "none";
  let animSpeed = 1;
  const rawAnim = rawSpec.anim;
  if (typeof rawAnim === "string") {
    animMode = pick(rawAnim, ["none", "flow", "step"] as const, "none");
  } else if (rawAnim && typeof rawAnim === "object") {
    const record = rawAnim as Record<string, unknown>;
    animMode = pick(record.mode, ["none", "flow", "step"] as const, "none");
    if (typeof record.speed === "number" && Number.isFinite(record.speed)) {
      animSpeed = Math.min(3, Math.max(0.5, record.speed));
    }
  }

  return {
    direction: pick(rawSpec.direction, DIRECTIONS, "LR"),
    anim: { mode: animMode, speed: animSpeed },
    nodes,
    edges,
    title: str(rawSpec.title, 200),
    name: /^[A-Za-z0-9._-]{1,64}$/.test(str(rawSpec.name, 64)) ? str(rawSpec.name, 64) : "",
  };
}

// ─── Palette ────────────────────────────────────────────────────────────────

/**
 * Resolved interface colours. Values are *baked* into the SVG (not `var()`)
 * because tone fills need alpha blending over the surface, and a baked figure
 * re-renders on a theme switch anyway — the mermaid model, not the svg model.
 */
export interface FlowPalette {
  font: string;
  surface: string;
  surface2: string;
  text: string;
  muted: string;
  border: string;
  accent: string;
  accentText: string;
  accentSoft: string;
  success: string;
  warning: string;
  danger: string;
}

/** Local copy of the token reader: importing `figureCard` back would be a cycle. */
function readToken(element: Element, name: string, fallback: string): string {
  const computed = window.getComputedStyle(element);
  return computed.getPropertyValue(name).trim() || fallback;
}

export function resolveFlowPalette(element: Element): FlowPalette {
  const computed = window.getComputedStyle(element);
  return {
    font: computed.fontFamily || "system-ui, sans-serif",
    surface: readToken(element, "--c-surface", "#ffffff"),
    surface2: readToken(element, "--c-surface-2", "#f4f4f4"),
    text: readToken(element, "--c-text", "#171a20"),
    muted: readToken(element, "--c-muted", "#5c5e62"),
    border: readToken(element, "--c-border-i", "#d0d1d2"),
    accent: readToken(element, "--c-accent", "#3e6ae1"),
    accentText: readToken(element, "--c-accent-text", "#3e6ae1"),
    accentSoft: readToken(element, "--c-accent-soft", "rgba(62, 106, 225, 0.10)"),
    success: readToken(element, "--c-success", "#1f8a3b"),
    warning: readToken(element, "--c-warning", "#b07500"),
    danger: readToken(element, "--c-danger", "#c0392b"),
  };
}

/**
 * Any CSS colour → `[r, g, b]`, through the same computed-style round trip the
 * browser itself does. Detached elements resolve nothing, so one hidden probe
 * node is kept in the document and recycled.
 */
let probe: HTMLDivElement | null = null;
function toRgb(value: string): [number, number, number] | null {
  probe ??= Object.assign(document.createElement("div"), { style: "position:absolute;visibility:hidden;pointer-events:none" });
  document.documentElement.append(probe);
  try {
    probe.style.color = "";
    probe.style.color = value;
    const match = /rgba?\(([^)]+)\)/.exec(window.getComputedStyle(probe).color);
    if (!match) return null;
    const parts = match[1].split(/[,/\s]+/).filter(Boolean).map(Number);
    if (parts.length < 3 || parts.some((n) => !Number.isFinite(n))) return null;
    return [parts[0], parts[1], parts[2]];
  } finally {
    probe.remove();
  }
}

/** Mix a colour over a base at `alpha` — the tint a tone node sits on. */
function tint(base: string, color: string, alpha: number): string {
  const b = toRgb(base);
  const c = toRgb(color);
  if (!b || !c) return base;
  const mix = b.map((v, i) => Math.round(v * (1 - alpha) + c[i] * alpha));
  return `rgb(${mix[0]}, ${mix[1]}, ${mix[2]})`;
}

// ─── Layout ─────────────────────────────────────────────────────────────────

const FONT_SIZE = 13;
const LINE_HEIGHT = 17;
const MAX_LINES = 3;
/** Longest label line before wrapping; the card is a column, not a canvas. */
const LABEL_MAX_WIDTH = 132;
const NODE_PAD_X = 22;
const NODE_PAD_Y = 14;
const CHIP_HEIGHT = 18;

interface Measured {
  lines: string[];
  width: number;
}

function measurer(font: string): (text: string, maxWidth: number) => Measured {
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  const measure = (text: string): number => {
    if (!ctx) return text.length * FONT_SIZE * 0.6;
    ctx.font = `${FONT_SIZE}px ${font}`;
    return ctx.measureText(text).width;
  };
  return (text: string, maxWidth: number): Measured => {
    // A CJK label has no spaces, so words longer than the box break by character.
    const words: string[] = [];
    for (const word of text.split(/\s+/).filter(Boolean)) {
      if (measure(word) <= maxWidth) {
        words.push(word);
        continue;
      }
      let run = "";
      for (const char of word) {
        if (run && measure(run + char) > maxWidth) {
          words.push(run);
          run = char;
        } else {
          run += char;
        }
      }
      if (run) words.push(run);
    }
    const lines: string[] = [];
    let line = "";
    for (const word of words) {
      const next = line ? `${line} ${word}` : word;
      if (line && measure(next) > maxWidth) {
        lines.push(line);
        line = word;
      } else {
        line = next;
      }
      if (lines.length === MAX_LINES) break;
    }
    if (line && lines.length < MAX_LINES) lines.push(line);
    // Overflow becomes an ellipsis on the last kept line, never a dropped word.
    if (words.length && lines.length === MAX_LINES && line && lines[MAX_LINES - 1] !== line) {
      let tail = lines[MAX_LINES - 1];
      while (tail && measure(`${tail}…`) > maxWidth) tail = tail.slice(0, -1);
      lines[MAX_LINES - 1] = `${tail}…`;
    }
    const width = Math.max(1, ...lines.map((l) => Math.ceil(measure(l))));
    return { lines: lines.length ? lines : [text], width };
  };
}

interface LayoutNode {
  id: string;
  label: string[];
  /** Centre in the laid-out canvas. */
  x: number;
  y: number;
  /** Drawn (uninflated) box, the shape's tight bounds. */
  w: number;
  h: number;
  shape: FlowShape;
  tone: FlowTone;
}

interface LayoutEdge {
  from: string;
  to: string;
  points: Array<{ x: number; y: number }>;
  label: string[];
  /** Measured text width — the chip's basis (a char-count estimate is not). */
  labelWidth: number;
  labelX: number;
  labelY: number;
}

interface FlowLayout {
  width: number;
  height: number;
  nodes: LayoutNode[];
  edges: LayoutEdge[];
}

/** Extra box the shape needs around its label for dagre to route around. */
function shapeInflate(shape: FlowShape, w: number, h: number): { w: number; h: number } {
  switch (shape) {
    case "decision":
      // A diamond keeps its label in the middle band: inflate both axes.
      return { w: w + 44, h: h + 38 };
    case "io":
      return { w: w + 26, h };
    case "database":
      return { w: w + 16, h: h + 16 };
    default:
      return { w, h };
  }
}

/** dagre lays the graph and returns centres + polyline points per edge. */
async function layoutFlow(spec: FlowSpec, palette: FlowPalette): Promise<FlowLayout | null> {
  const dagre = await loadDagre();
  const measure = measurer(palette.font);

  const nodes: Array<LayoutNode & { dw: number; dh: number }> = [];
  for (const node of spec.nodes) {
    const text = measure(node.label ?? node.id, LABEL_MAX_WIDTH);
    const w = Math.min(240, Math.max(64, text.width + NODE_PAD_X * 2));
    const h = Math.max(40, text.lines.length * LINE_HEIGHT + NODE_PAD_Y * 2);
    const inflated = shapeInflate(node.shape ?? "process", w, h);
    nodes.push({
      id: node.id,
      label: text.lines,
      x: 0,
      y: 0,
      w,
      h,
      shape: node.shape ?? "process",
      tone: node.tone ?? "default",
      dw: inflated.w,
      dh: inflated.h,
    });
  }

  const graph: DagreGraph = new dagre.Graph({ multigraph: true });
  graph.setGraph({
    rankdir: spec.direction ?? "LR",
    nodesep: 34,
    ranksep: 62,
    edgesep: 16,
    marginx: 10,
    marginy: 10,
  });
  graph.setDefaultNodeLabel(() => ({}));
  graph.setDefaultEdgeLabel(() => ({}));
  for (const node of nodes) graph.setNode(node.id, { width: node.dw, height: node.dh });

  const edges: Array<LayoutEdge> = [];
  spec.edges?.forEach((edge, index) => {
    const label = edge.label ? measure(edge.label, 110) : null;
    // The chip's size is given to dagre so it reserves room around the label.
    graph.setEdge(
      edge.from,
      edge.to,
      {
        width: label ? label.width + 12 : 1,
        height: label ? CHIP_HEIGHT : 1,
        labelpos: "c",
        labeloffset: 0,
        minlen: 1,
        weight: 1,
      },
      `e${index}`,
    );
    edges.push({
      from: edge.from,
      to: edge.to,
      points: [],
      label: label?.lines ?? [],
      labelWidth: label?.width ?? 0,
      labelX: 0,
      labelY: 0,
    });
  });

  dagre.layout(graph);

  const laid = new Map<string, { x: number; y: number }>();
  for (const node of nodes) {
    const placed = graph.node(node.id) as { x?: number; y?: number } | undefined;
    if (!placed || typeof placed.x !== "number" || typeof placed.y !== "number") return null;
    node.x = placed.x;
    node.y = placed.y;
    laid.set(node.id, { x: placed.x, y: placed.y });
  }

  const graphEdges = graph.edges();
  const byName = new Map(edges.map((_, index) => [`e${index}`, index]));
  graphEdges.forEach((graphEdge) => {
    const index = byName.get(graphEdge.name ?? "");
    if (index === undefined) return;
    const label = graph.edge(graphEdge) as { points?: Array<{ x: number; y: number }>; x?: number; y?: number };
    const entry = edges[index];
    entry.points = (label.points ?? []).map((p) => ({ x: p.x, y: p.y }));
    if (entry.points.length < 2) {
      const a = laid.get(entry.from);
      const b = laid.get(entry.to);
      if (a && b) entry.points = [a, b];
    }
    entry.labelX = typeof label.x === "number" ? label.x : 0;
    entry.labelY = typeof label.y === "number" ? label.y : 0;
  });

  const size = graph.graph() as { width?: number; height?: number };
  if (!size.width || !size.height) return null;
  return { width: size.width, height: size.height, nodes, edges };
}

// ─── Rendering ──────────────────────────────────────────────────────────────

const SVG_NS = "http://www.w3.org/2000/svg";
const XLINK_NS = "http://www.w3.org/1999/xlink";

function svgEl(name: string, attrs?: Record<string, string | number>): SVGElement {
  const element = document.createElementNS(SVG_NS, name);
  if (attrs) for (const [key, value] of Object.entries(attrs)) element.setAttribute(key, String(value));
  return element;
}

/** One decimal is all a coordinate needs; smaller markup, same picture. */
const n1 = (value: number): string => String(Math.round(value * 10) / 10);

/** Catmull–Rom → cubic Bézier: smooth curves through dagre's polyline. */
function smoothPath(points: Array<{ x: number; y: number }>): string {
  if (points.length === 0) return "";
  if (points.length === 1) return `M${n1(points[0].x)},${n1(points[0].y)}`;
  let d = `M${n1(points[0].x)},${n1(points[0].y)}`;
  for (let i = 0; i < points.length - 1; i += 1) {
    const p0 = points[Math.max(0, i - 1)];
    const p1 = points[i];
    const p2 = points[i + 1];
    const p3 = points[Math.min(points.length - 1, i + 2)];
    const c1x = p1.x + (p2.x - p0.x) / 6;
    const c1y = p1.y + (p2.y - p0.y) / 6;
    const c2x = p2.x - (p3.x - p1.x) / 6;
    const c2y = p2.y - (p3.y - p1.y) / 6;
    d += ` C${n1(c1x)},${n1(c1y)} ${n1(c2x)},${n1(c2y)} ${n1(p2.x)},${n1(p2.y)}`;
  }
  return d;
}

/** Polyline length of a laid-out edge, for the packet duration. */
function pathLength(points: Array<{ x: number; y: number }>): number {
  let length = 0;
  for (let i = 1; i < points.length; i += 1) {
    length += Math.hypot(points[i].x - points[i - 1].x, points[i].y - points[i - 1].y);
  }
  return length;
}

interface ToneColors {
  fill: string;
  stroke: string;
  label: string;
}

function toneColors(tone: FlowTone, palette: FlowPalette): ToneColors {
  switch (tone) {
    case "accent":
      return { fill: palette.accentSoft, stroke: palette.accent, label: palette.accentText };
    case "success":
      return { fill: tint(palette.surface, palette.success, 0.12), stroke: palette.success, label: palette.success };
    case "warning":
      return { fill: tint(palette.surface, palette.warning, 0.12), stroke: palette.warning, label: palette.warning };
    case "danger":
      return { fill: tint(palette.surface, palette.danger, 0.12), stroke: palette.danger, label: palette.danger };
    default:
      return { fill: palette.surface2, stroke: palette.border, label: palette.text };
  }
}

/** The shape body of one node, in its tight box (x/y are the centre). */
function shapeElement(node: LayoutNode, colors: ToneColors): SVGElement {
  const left = node.x - node.w / 2;
  const top = node.y - node.h / 2;
  const common = { fill: colors.fill, stroke: colors.stroke, "stroke-width": 1.25 };
  switch (node.shape) {
    case "start":
    case "end": {
      // Stadium: the semantic start/stop capsule of every flowchart notation.
      return svgEl("rect", { ...common, x: n1(left), y: n1(top), width: n1(node.w), height: n1(node.h), rx: n1(node.h / 2), ry: n1(node.h / 2) });
    }
    case "decision": {
      const inflated = shapeInflate(node.shape, node.w, node.h);
      const w = inflated.w;
      const h = inflated.h;
      return svgEl("polygon", {
        ...common,
        points: [
          `${n1(node.x)},${n1(node.y - h / 2)}`,
          `${n1(node.x + w / 2)},${n1(node.y)}`,
          `${n1(node.x)},${n1(node.y + h / 2)}`,
          `${n1(node.x - w / 2)},${n1(node.y)}`,
        ].join(" "),
      });
    }
    case "io": {
      const skew = 13;
      return svgEl("polygon", {
        ...common,
        points: [
          `${n1(left + skew)},${n1(top)}`,
          `${n1(left + node.w)},${n1(top)}`,
          `${n1(left + node.w - skew)},${n1(top + node.h)}`,
          `${n1(left)},${n1(top + node.h)}`,
        ].join(" "),
      });
    }
    case "database": {
      const ry = 7;
      const body = node.h - ry;
      return svgEl("path", {
        ...common,
        // Cylinder: straight sides, elliptical cap and base.
        d: [
          `M${n1(left)},${n1(top + ry)}`,
          `A${n1(node.w / 2)},${ry} 0 0 1 ${n1(left + node.w)},${n1(top + ry)}`,
          `L${n1(left + node.w)},${n1(top + ry + body - ry)}`,
          `A${n1(node.w / 2)},${ry} 0 0 1 ${n1(left)},${n1(top + ry + body - ry)}`,
          "Z",
        ].join(" "),
      });
    }
    default:
      return svgEl("rect", { ...common, x: n1(left), y: n1(top), width: n1(node.w), height: n1(node.h), rx: 6, ry: 6 });
  }
}

/** Internal stylesheet: only what a `var()` cannot reach and motion needs. */
function animStyle(anim: FlowAnim, reduced: boolean): string {
  if (anim.mode === "none" || reduced) return "";
  if (anim.mode === "flow") {
    const duration = n1(1.1 / (anim.speed ?? 1));
    return [
      "@keyframes flow-dash{to{stroke-dashoffset:-24}}",
      `.flow-dash{stroke-dasharray:5 7;animation:flow-dash ${duration}s linear infinite}`,
      "@media (prefers-reduced-motion: reduce){.flow-dash{animation:none;stroke-dasharray:none}}",
    ].join("");
  }
  return [
    "@keyframes flow-step{from{opacity:0.14}}",
    `.flow-step{animation:flow-step var(--t-dur,0.33s) var(--t-ease,ease-out) both}`,
    "@media (prefers-reduced-motion: reduce){.flow-step{animation:none;opacity:1}}",
  ].join("");
}

/**
 * Render one flowchart as a standalone `<svg>` element.
 *
 * Everything is baked: colours are resolved interface tokens, text is measured,
 * positions come from dagre. A theme switch re-renders (`figureCard.ts` holds
 * the redraw), the same model mermaid uses.
 *
 * Every `id` is namespaced per render: two flow figures in one reply each
 * define a marker and edge paths, and an unqualified `url(#flow-arrow)` would
 * make the second card's arrows resolve to the first card's marker.
 */
export async function renderFlowSvg(
  spec: FlowSpec,
  palette: FlowPalette,
  ariaLabel: string,
): Promise<SVGSVGElement | null> {
  const layout = await layoutFlow(spec, palette);
  if (!layout) return null;
  // One counter for the whole session: ids only need to be unique among the
  // live figures, and a monotonically growing suffix never collides.
  const uid = `flow-${(renderCounter += 1)}`;

  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const anim = spec.anim ?? { mode: "none" as const, speed: 1 };
  const moving = anim.mode === "flow" && !reduced;

  const svg = svgEl("svg", {
    viewBox: `0 0 ${n1(layout.width)} ${n1(layout.height)}`,
    role: "img",
    "aria-label": ariaLabel,
    xmlns: SVG_NS,
  }) as SVGSVGElement;

  // Arrow marker and motion CSS live in defs; marker colours are baked like
  // everything else, so the arrowhead follows the theme on re-render.
  const defs = svgEl("defs");
  const marker = svgEl("marker", {
    id: `${uid}-arrow`,
    viewBox: "0 0 10 10",
    refX: 9,
    refY: 5,
    markerWidth: 7,
    markerHeight: 7,
    orient: "auto-start-reverse",
  });
  marker.appendChild(svgEl("path", { d: "M0,0 L10,5 L0,10 Z", fill: palette.muted }));
  defs.appendChild(marker);
  const style = animStyle(anim, reduced);
  if (style) {
    const sheet = svgEl("style");
    sheet.textContent = style;
    defs.appendChild(sheet);
  }
  svg.appendChild(defs);

  // Edges first: nodes paint over the line ends, which is what hides the
  // half-arrow where a short edge would otherwise poke into a box.
  // One group per edge even when the path is empty: the motion overlays and
  // the step delays address these groups *by index*, so the sequence must not
  // shift when an edge is undrawable.
  const edgeGroup = svgEl("g");
  for (const [index, edge] of layout.edges.entries()) {
    const group = svgEl("g");
    const d = smoothPath(edge.points);
    if (d) {
      const path = svgEl("path", {
        id: `${uid}-edge-${index}`,
        d,
        fill: "none",
        stroke: palette.muted,
        "stroke-width": 1.5,
        "marker-end": `url(#${uid}-arrow)`,
      });
      group.appendChild(path);
    }

    if (edge.label.length > 0 && edge.labelWidth > 0) {
      // The chip is opaque so the line it covers disappears under it.
      const chipWidth = edge.labelWidth + 12;
      const chip = svgEl("rect", {
        x: n1(edge.labelX - chipWidth / 2),
        y: n1(edge.labelY - CHIP_HEIGHT / 2),
        width: n1(chipWidth),
        height: CHIP_HEIGHT,
        rx: 3,
        fill: palette.surface,
      });
      const text = svgEl("text", {
        x: n1(edge.labelX),
        y: n1(edge.labelY),
        "text-anchor": "middle",
        "dominant-baseline": "central",
        "font-family": palette.font,
        "font-size": 12,
        fill: palette.muted,
      });
      text.appendChild(svgEl("tspan")).textContent = edge.label.join(" ");
      group.append(chip, text);
    }
    edgeGroup.appendChild(group);
  }
  svg.appendChild(edgeGroup);

  const nodeGroup = svgEl("g");
  for (const node of layout.nodes) {
    const colors = toneColors(node.tone, palette);
    const group = svgEl("g");
    group.appendChild(shapeElement(node, colors));
    const count = node.label.length;
    node.label.forEach((line, lineIndex) => {
      const text = svgEl("text", {
        x: n1(node.x),
        y: n1(node.y + ((lineIndex - (count - 1) / 2) * LINE_HEIGHT) + 0.5),
        "text-anchor": "middle",
        "dominant-baseline": "central",
        "font-family": palette.font,
        "font-size": FONT_SIZE,
        fill: colors.label,
      });
      text.appendChild(svgEl("tspan")).textContent = line;
      group.appendChild(text);
    });
    nodeGroup.appendChild(group);
  }
  svg.appendChild(nodeGroup);

  // Motion overlays, both pure CSS/SMIL — the safe tier, no script.
  if (moving) {
    const overlay = svgEl("g");
    const speed = anim.speed ?? 1;
    for (const [index, edge] of layout.edges.entries()) {
      const source = edgeGroup.children[index]?.querySelector("path");
      const d = source?.getAttribute("d");
      if (!d) continue;
      const dash = svgEl("path", {
        d,
        class: "flow-dash",
        fill: "none",
        stroke: palette.accent,
        "stroke-width": 1.5,
        opacity: 0.55,
      });
      overlay.appendChild(dash);
      // One packet per edge: SMIL `animateMotion` along the same path, no JS.
      const duration = Math.max(1.2, pathLength(edge.points) / (95 * speed));
      const packet = svgEl("circle", { r: 2.6, fill: palette.accent });
      const motion = svgEl("animateMotion", { dur: `${n1(duration)}s`, repeatCount: "indefinite" });
      const href = svgEl("mpath");
      href.setAttribute("href", `#${uid}-edge-${index}`);
      href.setAttributeNS(XLINK_NS, "xlink:href", `#${uid}-edge-${index}`);
      motion.appendChild(href);
      packet.appendChild(motion);
      overlay.appendChild(packet);
    }
    svg.appendChild(overlay);
  }

  if (anim.mode === "step" && !reduced) {
    // Rank order is the story order: dagre already put sources before
    // targets, so the layout's own order is the reveal order.
    const horizontal = (spec.direction ?? "LR") === "LR";
    const ordered = [...layout.nodes].sort((a, b) =>
      horizontal ? a.x - b.x || a.y - b.y : a.y - b.y || a.x - b.x,
    );
    const delays = new Map(ordered.map((node, index) => [node.id, index * 0.28]));
    layout.nodes.forEach((node, index) => {
      const group = nodeGroup.children[index] as SVGElement | undefined;
      if (group) {
        group.classList.add("flow-step");
        group.style.animationDelay = `${n1(delays.get(node.id) ?? 0)}s`;
      }
    });
    layout.edges.forEach((edge, index) => {
      const group = edgeGroup.children[index] as SVGElement | undefined;
      if (!group) return;
      const delay = Math.max(delays.get(edge.from) ?? 0, delays.get(edge.to) ?? 0) + 0.33;
      group.classList.add("flow-step");
      group.style.animationDelay = `${n1(delay)}s`;
    });
  }

  return svg;
}

// ─── dagre loader ───────────────────────────────────────────────────────────

type DagreModule = typeof import("@dagrejs/dagre");

let dagreLoader: Promise<DagreModule> | null = null;
let renderCounter = 0;

/** Load dagre once; a separate chunk, off the startup path. */
function loadDagre(): Promise<DagreModule> {
  dagreLoader ??= import("@dagrejs/dagre");
  return dagreLoader;
}
