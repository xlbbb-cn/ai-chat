/**
 * PNG rasterisation for figures.
 *
 * Both figure kinds end up as a PNG in the same way — a canvas — but a 2D
 * figure is an SVG element, and an SVG file has no idea what the app's CSS
 * variables mean once it is serialised. So the element's *computed* styles are
 * inlined into the clone first: `fill="var(--c-accent)"`, `currentColor`, and
 * the inherited font all become literal values, and the saved image looks like
 * the one on screen instead of black-on-transparent.
 *
 * The saved file is 2× the displayed size (a plot pasted into a document
 * should be crisp), capped so a huge figure cannot allocate a 100-megapixel
 * canvas.
 */

/** Properties a standalone SVG cannot inherit from the app's stylesheet. */
const INLINE_PROPERTIES = [
  "fill",
  "fill-opacity",
  "fill-rule",
  "stroke",
  "stroke-opacity",
  "stroke-width",
  "stroke-linecap",
  "stroke-linejoin",
  "stroke-dasharray",
  "stroke-dashoffset",
  "opacity",
  "color",
  "font-family",
  "font-size",
  "font-weight",
  "font-style",
  "text-anchor",
  "dominant-baseline",
  "letter-spacing",
  "paint-order",
  "visibility",
];

const MAX_EDGE = 4096;

export interface PngOptions {
  /** Fill behind the drawing (the card's surface); omit for transparency. */
  background?: string;
  /** Backing-store multiplier; 2 keeps a pasted chart sharp. */
  scale?: number;
}

function stripDataUrl(dataUrl: string): string {
  const comma = dataUrl.indexOf(",");
  return comma === -1 ? dataUrl : dataUrl.slice(comma + 1);
}

/**
 * Copy the *computed* presentation of every element of `source` onto its twin
 * in `clone`. Both trees are walked in document order, so index `i` is the
 * same element on both sides.
 */
function inlineComputedStyles(source: Element, clone: Element): void {
  const origins = [source, ...Array.from(source.querySelectorAll("*"))];
  const copies = [clone, ...Array.from(clone.querySelectorAll("*"))];
  const count = Math.min(origins.length, copies.length);
  for (let i = 0; i < count; i += 1) {
    const computed = window.getComputedStyle(origins[i]);
    const target = copies[i] as SVGElement;
    const style = target.style;
    for (const property of INLINE_PROPERTIES) {
      const value = computed.getPropertyValue(property);
      if (value && value !== "normal") style.setProperty(property, value);
    }
  }
}

/** Rasterise a live `<svg>` element into base64 PNG bytes. */
export async function svgToPngBase64(
  svg: SVGSVGElement,
  options: PngOptions = {},
): Promise<string> {
  const clone = svg.cloneNode(true) as SVGSVGElement;
  inlineComputedStyles(svg, clone);

  const viewBox = (svg.getAttribute("viewBox") ?? "")
    .trim()
    .split(/[\s,]+/)
    .map((part) => Number.parseFloat(part));
  const bounds = svg.getBoundingClientRect();
  const box =
    viewBox.length === 4 && viewBox.every((value) => Number.isFinite(value))
      ? { x: viewBox[0], y: viewBox[1], width: viewBox[2], height: viewBox[3] }
      : { x: 0, y: 0, width: Math.max(1, bounds.width), height: Math.max(1, bounds.height) };
  if (box.width <= 0 || box.height <= 0) throw new Error("figure has no size");

  clone.setAttribute("width", String(box.width));
  clone.setAttribute("height", String(box.height));
  clone.removeAttribute("style");
  if (options.background) {
    const rect = clone.ownerDocument.createElementNS("http://www.w3.org/2000/svg", "rect");
    rect.setAttribute("x", String(box.x));
    rect.setAttribute("y", String(box.y));
    rect.setAttribute("width", String(box.width));
    rect.setAttribute("height", String(box.height));
    rect.setAttribute("fill", options.background);
    clone.insertBefore(rect, clone.firstChild);
  }

  const markup = new XMLSerializer().serializeToString(clone);
  // A data URL, not `btoa`: the drawing's labels can be non-Latin, and
  // `encodeURIComponent` handles UTF-8 without a manual escape dance.
  const url = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(markup)}`;
  const image = new Image();
  image.decoding = "sync";
  await new Promise<void>((resolve, reject) => {
    image.onload = () => resolve();
    image.onerror = () => reject(new Error("figure could not be rasterised"));
    image.src = url;
  });

  const scale = Math.min(
    options.scale ?? 2,
    MAX_EDGE / Math.max(box.width, box.height),
  );
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.round(box.width * scale));
  canvas.height = Math.max(1, Math.round(box.height * scale));
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("no 2D context");
  if (options.background) {
    ctx.fillStyle = options.background;
    ctx.fillRect(0, 0, canvas.width, canvas.height);
  }
  ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
  const png = stripDataUrl(canvas.toDataURL("image/png"));
  // Drop the backing store immediately: a 4000px figure is 64 MB otherwise.
  canvas.width = 0;
  canvas.height = 0;
  return png;
}

/** Base64 PNG bytes of a canvas the figure drew itself (`<carve>`). */
export function canvasToPngBase64(canvas: HTMLCanvasElement): string {
  if (canvas.width < 2 || canvas.height < 2) throw new Error("figure is empty");
  return stripDataUrl(canvas.toDataURL("image/png"));
}