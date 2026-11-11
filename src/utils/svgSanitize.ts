/**
 * Sanitiser for model-authored `<svg>` figures.
 *
 * The transcript never enables raw HTML: an assistant reply is rendered by
 * markdown-it with `html: false` and the only markup that reaches the DOM is
 * built here. A figure is a real SVG element, so it has to be pruned first —
 * an inline `<svg>` can carry `<script>`, event handlers, `<foreignObject>`
 * with arbitrary HTML, and `href="javascript:"` links.
 *
 * The rule is an element allowlist (drawing shapes, text, gradients, filters,
 * markers) plus an attribute blocklist: an unknown presentation attribute is
 * inert, while an unknown *element* can be a script container. `<style>` is
 * kept because drawings lean on it, but its text is screened for `@import`,
 * `expression()` and `url()` to anything but a fragment.
 */

/** Elements a figure may contain. Everything else is removed with its subtree. */
const ALLOWED_ELEMENTS = new Set([
  "svg",
  "g",
  "defs",
  "symbol",
  "use",
  "path",
  "rect",
  "circle",
  "ellipse",
  "line",
  "polyline",
  "polygon",
  "text",
  "tspan",
  "textpath",
  "title",
  "desc",
  "clippath",
  "mask",
  "marker",
  "pattern",
  "lineargradient",
  "radialgradient",
  "stop",
  "filter",
  "fegaussianblur",
  "feoffset",
  "femerge",
  "femergenode",
  "fecolormatrix",
  "feblend",
  "fecomposite",
  "feflood",
  "fedropshadow",
  "fetile",
  "femorphology",
  "feboxblur",
  "style",
]);

const SVG_NS = "http://www.w3.org/2000/svg";

/** A drawing far past this size is a freeze, not a figure. */
const MAX_ELEMENTS = 8000;

/** `url(#grad)` is a local reference; any other `url(...)` is a request. */
function hasExternalUrl(value: string): boolean {
  const local = value.replace(/url\(\s*(['"]?)#[^)]*\)/gi, "");
  return /url\s*\(/i.test(local);
}

function cssIsSafe(css: string): boolean {
  if (/@import|expression\s*\(|javascript:|behavior\s*:|-moz-binding/i.test(css)) return false;
  return !hasExternalUrl(css);
}

function keepAttribute(element: string, name: string, value: string): boolean {
  // Event handlers: the one attribute class that is code.
  if (name.startsWith("on")) return false;
  if (name === "href" || name === "xlink:href") return value.trim().startsWith("#");
  if (name === "src" || name === "srcset" || name === "data") return false;
  if (name === "style") return cssIsSafe(value);
  // A namespace declaration only belongs on the root element.
  if (name === "xmlns" || name.startsWith("xmlns:")) return element === "svg";
  if (/javascript:|data:text\/html|vbscript:/i.test(value)) return false;
  if (hasExternalUrl(value)) return false;
  return true;
}

/**
 * Parse and prune one SVG document.
 *
 * Returns standalone markup — a serialised `<svg>` element — or `null` when
 * the source is not a drawable SVG (the caller then shows the source as code
 * instead of dropping it silently).
 */
export function sanitizeSvg(source: string): string | null {
  if (typeof DOMParser === "undefined") return null;
  // A model often writes `<svg viewBox="…">` without `xmlns`. Parsed as XML
  // that root lands in *no* namespace, and every element would then be refused
  // as "unknown" — so the declaration is supplied before parsing.
  const markup = /^<svg\b(?![^>]*\bxmlns\s*=)/i.test(source.trim())
    ? source.trim().replace(/^<svg\b/i, `<svg xmlns="${SVG_NS}"`)
    : source;
  let doc: Document;
  try {
    doc = new DOMParser().parseFromString(markup, "image/svg+xml");
  } catch {
    return null;
  }
  if (doc.getElementsByTagName("parsererror").length > 0) return null;
  const root = doc.documentElement;
  if (!root || root.localName !== "svg" || root.namespaceURI !== SVG_NS) return null;

  // Without a viewBox there is nothing to scale into the card: derive one from
  // the declared size, or refuse.
  if (!root.getAttribute("viewBox")) {
    const width = Number.parseFloat(root.getAttribute("width") ?? "");
    const height = Number.parseFloat(root.getAttribute("height") ?? "");
    if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) {
      return null;
    }
    root.setAttribute("viewBox", `0 0 ${width} ${height}`);
  }

  const all = [root, ...Array.from(root.querySelectorAll("*"))];
  if (all.length > MAX_ELEMENTS) return null;

  for (const element of all) {
    const name = element.localName.toLowerCase();
    if (!ALLOWED_ELEMENTS.has(name)) {
      element.remove();
      continue;
    }
    for (const attr of Array.from(element.attributes)) {
      if (!keepAttribute(name, attr.name.toLowerCase(), attr.value)) {
        element.removeAttribute(attr.name);
      }
    }
    if (name === "style" && !cssIsSafe(element.textContent ?? "")) element.remove();
  }

  // A root that lost its only child to the allowlist is an empty frame.
  if (root.childElementCount === 0) return null;

  const serialised = new XMLSerializer().serializeToString(root);
  return serialised.length > 0 ? serialised : null;
}