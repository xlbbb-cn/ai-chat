/**
 * `<html-fig>` — a dynamic figure written in HTML5.
 *
 * An `svg` drawing is static and a `carve` scene is declarative: neither can
 * show motion. A *dynamic* figure (a physics sketch that moves, a plot the user
 * can drag, a canvas animation) needs real HTML + CSS + script — which is
 * exactly what the transcript must never put in its own document.
 *
 * So the fragment is not executed here. It is assembled into a complete
 * document and handed to an `<iframe sandbox="allow-scripts">`, which is the
 * one place in a webview where markup *and* script can run without touching the
 * app:
 *
 * - `sandbox` without `allow-same-origin` gives the frame an **opaque origin**:
 *   `parent.document` throws, `localStorage` throws, no cookie of the app is
 *   readable, and no `postMessage` can be spoofed into app code that does not
 *   check `event.source`. Verified against WebView2/Chromium behaviour.
 * - `allow-scripts` is the *only* permission: no top navigation, no popups, no
 *   forms, no modals, no pointer lock. The frame cannot navigate the app away.
 * - A `Content-Security-Policy` meta pins the network shut (`default-src
 *   'none'`, so no `connect-src`, no `img-src` off `data:`, no external script
 *   or stylesheet): every request a fragment makes is refused. Measured on
 *   WebKitGTK — fourteen channels tried (`fetch`, `XHR`, beacon, WebSocket,
 *   `EventSource`, `img`, `script`, stylesheet, `@import`, `@font-face`,
 *   `url()`, media, child frame), zero requests reached a listening server. The
 *   app document itself ships without a CSP (`csp: null` in `tauri.conf.json`),
 *   so a `srcdoc` frame inherits nothing — this policy is the sandbox's second
 *   wall, not a duplicate.
 * - **Navigation is the one channel CSP does not govern** (`navigate-to` never
 *   shipped): a fragment that assigns `location` gets its request out, and the
 *   URL is the payload. The sandbox still applies to the arriving document, but
 *   `figureCard.ts` therefore counts frame loads and closes a figure that leaves
 *   the document it was given. On WebKitGTK that frame also shares the web
 *   process with the transcript — a `while (true) {}` freezes the whole window —
 *   which is why the frame is only created after the reader asks for it.
 *
 * Two channels cross the boundary, both parent→frame or frame→parent messages:
 *
 * - **Theme**: CSS custom properties do not inherit through a frame element, so
 *   the app's tokens are baked into the frame's `:root` when it is built, and
 *   re-sent as a message on a theme / palette switch. Sending beats re-writing
 *   `srcdoc`: the animation keeps running instead of restarting.
 * - **Height**: a frame cannot size itself, so a small reporter inside the
 *   wrapper measures the content and posts the height; the card clamps it.
 *
 * The wrapper script is the *only* script this module authors; everything else
 * in the document came from the model, inside the sandbox.
 */

/** Tokens a dynamic figure may use. A name outside this list is not projected. */
const TOKEN_NAMES: Array<[string, string]> = [
  ["--c-bg", "#FFFFFF"],
  ["--c-surface", "#FFFFFF"],
  ["--c-surface-2", "#F4F4F4"],
  ["--c-text", "#171A20"],
  ["--c-body", "#393C41"],
  ["--c-muted", "#5C5E62"],
  ["--c-border", "#EEEEEE"],
  ["--c-border-s", "#F4F4F4"],
  ["--c-border-i", "#D0D1D2"],
  ["--c-accent", "#3E6AE1"],
  ["--c-accent-text", "#3E6AE1"],
  ["--c-accent-soft", "rgba(62, 106, 225, 0.10)"],
  ["--c-on-accent", "#FFFFFF"],
  ["--c-success", "#1F8A3B"],
  ["--c-warning", "#B07500"],
  ["--c-danger", "#C0392B"],
  ["--c-code", "#F4F4F4"],
];

/** Largest fragment accepted (bytes of source). Beyond it, the source is shown. */
export const MAX_HTML_SOURCE = 256 * 1024;

/** Frame height bounds: a card never collapses to a line, never eats the reply. */
export const MIN_FRAME_HEIGHT = 120;
export const MAX_FRAME_HEIGHT = 560;

/** Message tag both sides of the frame boundary agree on. */
export const FRAME_MESSAGE_TAG = 1;

/**
 * The frame's policy. `default-src 'none'` closes every fetch-like channel;
 * inline style and script stay open because that is what a dynamic figure *is*.
 * A wider opener would be a different feature, not a bigger picture.
 */
const CSP = [
  "default-src 'none'",
  "img-src data: blob:",
  "media-src data: blob:",
  "font-src data:",
  "style-src 'unsafe-inline'",
  "script-src 'unsafe-inline'",
  "base-uri 'none'",
  "form-action 'none'",
  "frame-src 'none'",
].join("; ");

/**
 * The frame sheet: the card's own recipe (content on `--c-surface`, 13px body
 * text) plus a zeroed margin so the measured height is the drawing's height.
 * Colours are tokens only — the values arrive from the app, per theme.
 */
const BASE_CSS = `html,body{margin:0;padding:0}
body{padding:12px;box-sizing:border-box;background:var(--c-surface);color:var(--c-text);
font-family:var(--fig-font,system-ui,sans-serif);font-size:13px;line-height:1.5;
overflow-wrap:break-word;-webkit-text-size-adjust:100%}
*,*::before,*::after{box-sizing:border-box}
img,canvas,video,svg{max-width:100%}
button,input,select,textarea{font:inherit;color:inherit}
:focus-visible{outline:2px solid var(--c-accent);outline-offset:2px}
`;

/**
 * The wrapper's reporter. Runs before the fragment's own scripts so a figure
 * that throws still reports a height.
 *
 * `body`'s border box — not `documentElement` — is the content height: the root
 * element fills the frame viewport, so measuring it would just echo whatever
 * height the card set and the message would be a loop.
 */
const REPORTER = `(function(){
var body=null;
function report(){
  var target=body||document.body;
  if(!target)return;
  var height=Math.ceil(target.getBoundingClientRect().height);
  try{parent.postMessage({htmlFig:${FRAME_MESSAGE_TAG},height:height},"*");}catch(e){}
}
function adopt(event){
  var data=event.data;
  if(!data||data.htmlFigTheme!==${FRAME_MESSAGE_TAG}||!data.tokens)return;
  var root=document.documentElement;
  for(var name in data.tokens){
    if(Object.prototype.hasOwnProperty.call(data.tokens,name))root.style.setProperty(name,data.tokens[name]);
  }
  report();
}
body=document.body;
try{new ResizeObserver(report).observe(document.body);}catch(e){}
addEventListener("message",adopt);
addEventListener("load",report);
addEventListener("error",report,true);
setTimeout(report,0);
report();
})();`;

export interface FigureTokens {
  [name: string]: string;
}

/** One app token, with a single `var(--x)` hop resolved (palettes do that). */
function readVar(computed: CSSStyleDeclaration, name: string, fallback: string): string {
  let value = computed.getPropertyValue(name).trim();
  const indirect = /^var\(\s*(--[A-Za-z0-9-]+)\s*\)$/.exec(value);
  if (indirect) value = computed.getPropertyValue(indirect[1]).trim();
  return value || fallback;
}

/** The tokens a frame is built with — read live, so a palette switch follows. */
export function readFigureTokens(element: Element): FigureTokens {
  // One style read for the whole sheet: seventeen `getComputedStyle` calls would
  // each force the same style resolution.
  const computed = window.getComputedStyle(element);
  const tokens: FigureTokens = {};
  for (const [name, fallback] of TOKEN_NAMES) tokens[name] = readVar(computed, name, fallback);
  // The frame is a separate document: it cannot inherit the app's typeface, and
  // a system stack would make the drawing look like a different app.
  tokens["--fig-font"] = computed.fontFamily || "system-ui, sans-serif";
  return tokens;
}

function tokenBlock(tokens: FigureTokens): string {
  const body = Object.keys(tokens)
    .filter((name) => Boolean(tokens[name]))
    .map((name) => `${name}:${tokens[name]};`)
    .join("");
  return `:root{${body}}`;
}

/** Attributes the fragment's own `<head>` cannot be trusted to keep. */
const DROPPED_HEAD_TAGS = new Set(["BASE", "LINK", "META", "TITLE"]);

/**
 * Break apart a closing tag in text the serialiser emits verbatim.
 *
 * A belt, not a fix: the sequence is unreachable today. A literal `</style>` /
 * `</script>` ends the element during the first parse, and raw-text elements do
 * not decode entities — `&lt;/style&gt;` stays text and closes nothing (checked
 * in the browser) — so no text node the parser produces can hold one. It stays
 * because the serialiser would emit it verbatim if this module ever started
 * writing text into a `<style>` or `<script>` itself, and because the frame's
 * reporter is the only thing a breakout could cost.
 */
function defuseRawText(doc: Document): void {
  for (const element of Array.from(doc.querySelectorAll("style, script"))) {
    const inner = element.textContent ?? "";
    if (!/<\/(?:style|script)/i.test(inner)) continue;
    element.textContent = inner.replace(/<\/(style|script)/gi, "<\\/$1");
  }
}

interface DocumentOptions {
  /** Bake the height reporter in (the live card) or leave it out (a saved file). */
  live: boolean;
  title?: string;
}

/**
 * Assemble the frame document.
 *
 * The fragment is parsed once and its nodes are *imported*, never concatenated
 * into a template string: a string build would let a `</style>` inside the
 * model's CSS reopen the wrapper's markup. The result is one standalone
 * document, which is also what a saved `.html` file holds.
 */
export function buildFigureDocument(
  source: string,
  tokens: FigureTokens,
  options: DocumentOptions,
): string {
  const parsed = new DOMParser().parseFromString(source, "text/html");
  const doc = document.implementation.createHTMLDocument(options.title ?? "");

  const charset = doc.createElement("meta");
  charset.setAttribute("charset", "utf-8");
  const policy = doc.createElement("meta");
  policy.setAttribute("http-equiv", "Content-Security-Policy");
  policy.setAttribute("content", CSP);
  const style = doc.createElement("style");
  style.textContent = `${tokenBlock(tokens)}\n${BASE_CSS}`;
  const viewport = doc.createElement("meta");
  viewport.setAttribute("name", "viewport");
  viewport.setAttribute("content", "width=device-width, initial-scale=1");
  doc.head.append(charset, policy, viewport, style);

  // A fragment's `<style>` may land in `<head>` or `<body>` depending on where
  // the model wrote it; both are hoisted, because a body-level `<style>` is
  // still document-wide and hoisting keeps cascade order stable.
  for (const element of Array.from(parsed.querySelectorAll("style"))) {
    doc.head.appendChild(doc.importNode(element, true));
    // Removed from the source tree, so a `<style>` nested inside a wrapper does
    // not stay behind and apply its rules twice.
    element.remove();
  }
  // Anything else the fragment's head holds is dropped: `base`/`link`/`meta`
  // point at the network (blocked anyway), and a stray `title` would rename a
  // saved file.
  if (options.live) {
    const reporter = doc.createElement("script");
    reporter.textContent = REPORTER;
    doc.body.appendChild(reporter);
  }
  // Head scripts move into the body, in order, after the reporter: a script that
  // ran in `<head>` before the body existed would see an empty document.
  for (const element of Array.from(parsed.head.children)) {
    if (DROPPED_HEAD_TAGS.has(element.tagName) || element.tagName === "STYLE") continue;
    doc.body.appendChild(doc.importNode(element, true));
  }
  for (const element of Array.from(parsed.body.childNodes)) {
    if ((element as Element).tagName === "STYLE") continue;
    doc.body.appendChild(doc.importNode(element, true));
  }
  defuseRawText(doc);
  return `<!DOCTYPE html>\n${doc.documentElement.outerHTML}\n`;
}

/** The live card's document (reporter in, no title). */
export function buildFrameDocument(source: string, tokens: FigureTokens): string {
  return buildFigureDocument(source, tokens, { live: true });
}

/**
 * The document a save writes: the same figure, self-contained.
 *
 * The reporter is dropped (there is no parent to report to) and the tokens are
 * baked, so double-clicking the file shows what was on screen. The CSP stays —
 * a saved figure has no reason to reach the network either.
 */
export function buildStandaloneDocument(
  source: string,
  tokens: FigureTokens,
  title: string,
): string {
  return buildFigureDocument(source, tokens, { live: false, title });
}

/** Re-theme a live frame without rebuilding it: the animation keeps running. */
export function postFigureTokens(frame: HTMLIFrameElement, tokens: FigureTokens): void {
  const target = frame.contentWindow;
  if (!target) return;
  try {
    target.postMessage({ htmlFigTheme: FRAME_MESSAGE_TAG, tokens }, "*");
  } catch {
    /* the frame went away between the read and the post */
  }
}

/**
 * Whether the fragment would run script at all.
 * The tiers in `figureCard.ts` turn on this answer: markup, CSS and SVG
 * animation cannot block the thread they share with the transcript, but a
 * `while (true) {}` freezes the whole window — so a fragment that *could* run
 * script is the one that needs consent. A `<script>` element is the obvious
 * carrier; an `on…` attribute is the invisible one (the handler only fires on an
 * event, but it fires on the same thread), and a `javascript:` URL is the third
 * spelling of the same thing.
 */
export function hasInlineScript(source: string): boolean {
  if (!/<script|\son[a-z]+\s*=|javascript:/i.test(source)) return false;
  try {
    const parsed = new DOMParser().parseFromString(source, "text/html");
    if (parsed.querySelector("script")) return true;
    for (const element of Array.from(parsed.querySelectorAll("*"))) {
      for (const attribute of Array.from(element.attributes)) {
        const name = attribute.name.toLowerCase();
        const value = attribute.value.trim().toLowerCase();
        if (name.startsWith("on") && name.length > 2) return true;
        if (
          (name === "href" || name === "src" || name === "action" || name === "formaction") &&
          value.startsWith("javascript:")
        ) {
          return true;
        }
      }
    }
    return false;
  } catch {
    // A fragment we cannot parse is one we cannot clear, so it takes the gate.
    return true;
  }
}
