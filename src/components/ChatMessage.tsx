import type { BackgroundTaskState, ChatInjection, Message, MessageContent } from "../types";
import {
  Component,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { BackgroundTaskGroup } from "./BackgroundTaskGroup";
import { ToolCallGroup } from "./ToolCallGroup";
import { TodoList } from "./TodoList";
import MarkdownIt from "markdown-it";
import hljs from "highlight.js";
import "highlight.js/styles/github.css";
import "katex/dist/katex.min.css";
import katexCssUrl from "katex/dist/katex.min.css?url";
import razerF5FontUrl from "../assets/Web Fonts/9b77a61b70b873a0f298ad6fbc801666.woff2?url";
import { katexMathPlugin } from "../utils/markdownMath";
import { extractFigures, type FigureSpec } from "../utils/figures";
import { mountFigures, saveAllFigures } from "../utils/figureCard";
import {
  extractDiagrams,
  inlineDiagramsForPrint,
  mountDiagrams,
  type DiagramSpec,
} from "../utils/mermaidDiagrams";
import { mentionDisplayName } from "../composerFiles";
// `t` (module-level) is used by the markdown-it renderer rules, which run
// outside the React tree; the hook keeps the component re-rendering on change.
import { t as text, useI18n } from "../i18n";
import {
  FontAwesomeIcon,
  faCodeBranch,
  faFileArrowDown,
  faImages,
  faPen,
  faPenToSquare,
  faRotateRight,
  faTrash,
} from "../icons";
import "./ChatMessage.css";

import { writeText as tauriWriteText } from "@tauri-apps/plugin-clipboard-manager";

const escapeHtml = (value: string): string =>
  value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\"/g, "&quot;")
    .replace(/'/g, "&#39;");

/**
 * Image sources a reply may embed.
 *
 * A markdown image is the one construct that turns text into a live element
 * that fetches something, so it is limited to what cannot reach a local file:
 * http(s) and an inline raster. Anything else is shown as its alt text.
 */
const SAFE_IMAGE_SRC = /^(?:https?:\/\/|data:image\/(?:png|jpe?g|gif|webp|bmp);base64,)/i;

/**
 * Build a markdown renderer.
 *
 * Two profiles exist because the transcript hosts two different kinds of text:
 *
 * * a **reply** may draw (fenced code, KaTeX, a remote image inside the safe
 *   source list);
 * * what a **user typed** must stay literal, so image syntax is not a rule at
 *   all — typing `![a](b)` shows those characters instead of injecting an
 *   `<img>` into the user's own bubble. Raw HTML is off for both.
 */
function createMarkdown(profile: { images: boolean }): MarkdownIt {
  const instance = new MarkdownIt({
    // Disable raw HTML and auto-linking to prevent navigation vectors.
    html: false,
    linkify: false,
    typographer: true,
    breaks: true,
  });

  instance.renderer.rules.fence = (tokens, idx) => {
    const token = tokens[idx];
    const info = token.info ? token.info.trim().split(/\s+/g)[0] : "";
    const language = info && hljs.getLanguage(info) ? info : "";
    const code = escapeHtml(token.content);
    const highlighted = language
      ? hljs.highlight(token.content, { language }).value
      : code;
    const className = language
      ? `language-${instance.utils.escapeHtml(language)}`
      : "";

    return `
      <div class="code-block-wrapper">
        <div class="code-block-toolbar">
          <button type="button" class="code-copy-btn">${escapeHtml(text("common.copy"))}</button>
        </div>
        <pre class="hljs"><code class="${className}">${highlighted}</code></pre>
      </div>
    `;
  };

  // Remove markdown link parsing entirely ([text](url), autolink) and strip any
  // fallback anchor tokens.
  instance.disable(["link", "autolink"]);
  instance.renderer.rules.link_open = () => "<span>";
  instance.renderer.rules.link_close = () => "</span>";

  if (profile.images) {
    // `image` is the one markdown rule that emits a live element from text, so
    // its source is vetted here instead of relying on link validation.
    instance.renderer.rules.image = (tokens, idx, options, _env, self) => {
      const token = tokens[idx];
      const src = token.attrGet("src") ?? "";
      const alt = token.content ?? "";
      if (!SAFE_IMAGE_SRC.test(src)) return escapeHtml(alt || src);
      token.attrSet("alt", alt);
      token.attrSet("loading", "lazy");
      token.attrSet("referrerpolicy", "no-referrer");
      token.attrJoin("class", "message-image");
      return self.renderToken(tokens, idx, options);
    };
  } else {
    instance.disable("image");
  }

  // Render `$...$` / `$$...$$` formulas with KaTeX.
  instance.use(katexMathPlugin);
  return instance;
}

/** Reply body: may draw, may embed a vetted image. */
const md = createMarkdown({ images: true });
/**
 * User body: the same text rules without the one rule that manufactures HTML,
 * so a typed symbol can never become an element.
 */
const mdLiteral = createMarkdown({ images: false });

/**
 * Render markdown without letting a renderer failure escape.
 *
 * markdown-it, highlight.js and KaTeX are all driven by the message text, and a
 * throw here would blank the whole transcript (React unmounts the tree on an
 * uncaught render error). A failure therefore degrades to the escaped literal
 * text: the message stays readable and nothing it contains becomes markup.
 */
function renderMarkdown(instance: MarkdownIt, source: string): string {
  try {
    return instance.render(source);
  } catch (error) {
    console.error("Markdown rendering failed", error);
    return `<pre class="markdown-fallback">${escapeHtml(source)}</pre>`;
  }
}

function buildPrintableMessageHtml(contentHtml: string, exportedAt: string): string {
  return `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>${escapeHtml(exportedAt)}</title>
    <link rel="stylesheet" href="${escapeHtml(katexCssUrl)}" data-katex-css />
    <style>
      @font-face {
        font-family: "RazerF5";
        src: url("${razerF5FontUrl}") format("woff2");
        font-weight: 400;
        font-style: normal;
      }

      :root {
        color-scheme: light;
        font-family: "RazerF5", "Segoe UI", "Microsoft YaHei", sans-serif;
        /* A figure's SVG is written in theme tokens; this document is its own
           document, so it needs the light values those names stand for. */
        --c-surface: #ffffff;
        --c-surface-2: #f6f7f8;
        --c-bg: #ffffff;
        --c-text: #171a20;
        --c-body: #2b2f36;
        --c-muted: #5c5e62;
        --c-border-s: #e8eaed;
        --c-border-i: #d9dce1;
        --c-accent: #3e6ae1;
        --c-success: #1f8a3b;
        --c-warning: #b07500;
        --c-danger: #c0392b;
      }

      * {
        box-sizing: border-box;
      }

      body {
        margin: 0;
        padding: 32px;
        background: #ffffff;
        color: #171a20;
        font-size: 14px;
        line-height: 1.6;
        word-break: break-word;
      }

      main {
        width: 100%;
      }

      p {
        margin: 0 0 0.75em;
      }

      ul,
      ol {
        margin: 0 0 0.75em;
        padding-left: 1.5em;
      }

      li {
        margin-bottom: 0.25em;
      }

      pre {
        margin: 0.75em 0;
        padding: 12px 14px;
        background: #f4f4f4;
        border: 1px solid #e5e7eb;
        border-radius: 8px;
        overflow-x: auto;
        white-space: pre-wrap;
      }

      code {
        font-family: "RazerF5", "Cascadia Code", "Consolas", "Microsoft YaHei", monospace;
      }

      table {
        width: 100%;
        border-collapse: collapse;
        margin: 0.75em 0;
      }

      th,
      td {
        border: 1px solid #d1d5db;
        padding: 8px 10px;
        text-align: left;
        vertical-align: top;
      }

      th {
        background: #f3f4f6;
      }

      .code-block-toolbar {
        display: none !important;
      }

      .chat-figure {
        margin: 0.75em 0;
        break-inside: avoid;
      }

      .chat-figure-viewport {
        padding: 8px;
        background: #ffffff;
        border: 1px solid var(--c-border-s);
        border-radius: 4px;
      }

      .chat-figure svg,
      .chat-figure img {
        display: block;
        width: 100%;
        height: auto;
      }

      /* Same cap as the transcript: a huge viewBox letterboxes instead of
         printing hundreds of near-empty pages. */
      .chat-figure svg {
        max-height: 420px;
      }

      /* A dynamic figure prints as the frame it is; the height was measured
         live and pinned on the clone, so the page is laid out before the
         fragment finishes loading. */
      .chat-figure-frame {
        display: block;
        width: 100%;
        border: 0;
        break-inside: avoid;
      }

      .chat-figure-bar {
        margin-top: 4px;
        font-size: 11px;
        color: var(--c-muted);
      }

      /* Nothing in a printed page is clickable. */
      .chat-figure-actions,
      .chat-figure-status {
        display: none !important;
      }
    </style>
  </head>
  <body>
    <main>${contentHtml}</main>
  </body>
</html>`;
}

/**
 * The reply as standalone HTML for print / PDF.
 *
 * The markdown alone is not enough: figure cards are mounted imperatively, and
 * a 3D figure is a `<canvas>` — which a document write cannot carry. So the
 * live subtree is cloned and every canvas is frozen into an `<img>` of its own
 * pixels; a 2D figure stays vector and picks the token values up from the
 * print stylesheet.
 *
 * A Mermaid diagram is already an `<svg>` in the live subtree, so the clone
 * carries it. It is re-rendered in the printed page's own palette, though: the
 * diagram on screen has the app's colours baked in, and a dark-mode reply would
 * otherwise print light text on a dark block on a white sheet.
 */
async function printableMessageHtml(root: HTMLElement | null, fallback: string): Promise<string> {
  if (!root) return fallback;
  const clone = root.cloneNode(true) as HTMLElement;
  const canvases = Array.from(root.querySelectorAll<HTMLCanvasElement>(".chat-figure-canvas"));
  if (canvases.length > 0) {
    const copies = Array.from(clone.querySelectorAll<HTMLCanvasElement>(".chat-figure-canvas"));
    canvases.forEach((canvas, i) => {
      const image = document.createElement("img");
      image.className = "chat-figure-canvas";
      image.alt = "";
      try {
        image.src = canvas.toDataURL("image/png");
      } catch {
        return;
      }
      copies[i]?.replaceWith(image);
    });
  }
  // A dynamic figure is a frame, and a clone carries only its attributes: the
  // print page reloads the fragment on its own. Its height was measured live, so
  // it is pinned here — otherwise the card would print at the CSS floor and clip
  // the drawing.
  const frames = Array.from(root.querySelectorAll<HTMLIFrameElement>(".chat-figure-frame"));
  if (frames.length > 0) {
    const copies = Array.from(clone.querySelectorAll<HTMLIFrameElement>(".chat-figure-frame"));
    frames.forEach((frame, i) => {
      const copy = copies[i];
      if (!copy) return;
      copy.setAttribute("height", String(Math.round(frame.getBoundingClientRect().height)));
    });
  }
  try {
    await inlineDiagramsForPrint(root, clone);
  } catch (error) {
    // The clone already holds whatever was on screen; a failed re-render must
    // not cost the user the export.
    console.error("Diagram export failed", error);
  }
  return clone.innerHTML;
}

function exportAssistantMessagePdf(contentHtml: string, messageId: string) {
  const iframe = document.createElement("iframe");
  const exportedAt = `assistant-message-${messageId}`;

  iframe.style.position = "fixed";
  iframe.style.right = "0";
  iframe.style.bottom = "0";
  iframe.style.width = "0";
  iframe.style.height = "0";
  iframe.style.border = "0";
  iframe.setAttribute("aria-hidden", "true");

  const cleanup = () => {
    iframe.remove();
  };

  iframe.addEventListener("load", () => {
    const frameWindow = iframe.contentWindow;
    const frameDocument = frameWindow?.document;
    if (!frameWindow || !frameDocument) {
      cleanup();
      return;
    }

    frameDocument.open();
    frameDocument.write(buildPrintableMessageHtml(contentHtml, exportedAt));
    frameDocument.close();

    const handleAfterPrint = () => {
      frameWindow.removeEventListener("afterprint", handleAfterPrint);
      window.setTimeout(cleanup, 0);
    };

    frameWindow.addEventListener("afterprint", handleAfterPrint);

    // Wait for the KaTeX stylesheet before printing so formulas are laid out.
    let printed = false;
    const doPrint = () => {
      if (printed) return;
      printed = true;
      frameWindow.focus();
      frameWindow.print();
    };

    const katexLink = frameDocument.querySelector<HTMLLinkElement>("link[data-katex-css]");
    if (katexLink) {
      katexLink.addEventListener("load", doPrint, { once: true });
      katexLink.addEventListener("error", doPrint, { once: true });
      // Fallback in case neither event fires (e.g. served from cache).
      window.setTimeout(doPrint, 400);
    } else {
      window.setTimeout(doPrint, 50);
    }
  }, { once: true });

  document.body.appendChild(iframe);
}

interface Props {
  message: Message;
  showRetry?: boolean;
  onRetry?: () => void;
  onDelete?: (messageId: string) => void;
  onFork?: (messageId: string) => void;
  dbId?: number;
  /** Inline allow/deny for a pending `run_shell` confirmation in the tool-call group. */
  onConfirm?: (requestId: string, allowed: boolean) => void;
  /** Stop a running background tool task (its process tree is terminated). */
  onCancelBackground?: (taskId: string) => void;
  /**
   * Live background work for this turn. Only the newest assistant message gets
   * it, so the group is drawn once — between the todo list and the body.
   */
  backgroundTasks?: BackgroundTaskState[];
  backgroundWaiting?: number;
  /**
   * Skill names in the workspace. A `/name` token in a user message is only
   * highlighted when it names one of these, so a path such as `/usr/bin` or a
   * `1/2` fraction stays plain text.
   */
  skillNames?: string[];
  /**
   * Agent tokens (`+agent`, without the `+`) known in the roster. A `+token`
   * in a user message is highlighted only when it names one of them, so
   * arithmetic such as `2 + 2` and `C++` stays plain text.
   */
  agentTokens?: string[];
  /** Session this message belongs to — the todo panel fetches its active list. */
  sessionId?: string;
  /**
   * Mirror the session's todo list under this message. Only the newest
   * assistant message gets it, so the list is drawn once per session.
   */
  todoActive?: boolean;
}

/** Extract the textual portion of a multimodal `MessageContent`. */
function contentToText(content: MessageContent): string {
  if (typeof content === "string") return content;
  const parts: string[] = [];
  for (const part of content) {
    if (part.type === "text") {
      parts.push(part.text);
    }
  }
  return parts.join("");
}

/**
 * Wrap `@file#lines` mentions in the body with an accent span.
 *
 * A mention is how the user references an attached file, so it reads as a
 * reference rather than as prose — the accent is the same colour a link
 * already wears. Code, formulas and links are skipped: there an `@` is
 * literal text that must not be recoloured.
 */
function highlightFileMentions(root: HTMLElement) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  while (walker.nextNode()) nodes.push(walker.currentNode as Text);

  // Mirrors `parseFileMentions` in `fileMention.ts`: `@path` (or `@@name` for a
  // file in the workspace tmp folder), then an optional `#12` / `#12-24` /
  // `#12-` range.
  const token = /@@?[^\s@#<>]+(?:#\d+(?:-\d*)?)?/g;

  for (const node of nodes) {
    const text = node.data;
    if (!text.includes("@")) continue;
    if (node.parentElement?.closest("pre, code, a, .katex, button")) continue;

    const fragment = document.createDocumentFragment();
    let cursor = 0;
    let found = false;
    token.lastIndex = 0;
    let match: RegExpExecArray | null;
    while ((match = token.exec(text)) !== null) {
      // Only a token that opens the line or follows whitespace is a mention.
      if (match.index > 0 && !/\s/.test(text[match.index - 1])) continue;
      found = true;
      if (match.index > cursor) {
        fragment.appendChild(document.createTextNode(text.slice(cursor, match.index)));
      }
      const span = document.createElement("span");
      span.className = "file-mention-token";
      span.textContent = match[0];
      fragment.appendChild(span);
      cursor = match.index + match[0].length;
    }
    if (!found) continue;
    if (cursor < text.length) {
      fragment.appendChild(document.createTextNode(text.slice(cursor)));
    }
    node.replaceWith(fragment);
  }
}

/**
 * Wrap `/skill-name` mentions in the body with the same accent span `@file`
 * mentions wear — a slash token is a reference to a workspace skill, not prose.
 *
 * Mirrors `activeSlashQuery` in `slashMention.ts`: the `/` must open the line or
 * follow whitespace. A token only counts when it names a known skill (see
 * `skillNames`), so paths (`/usr/bin`) and fractions stay untouched, and
 * trailing punctuation is left outside the token.
 */
function highlightSkillMentions(root: HTMLElement, known: Set<string>) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  while (walker.nextNode()) nodes.push(walker.currentNode as Text);

  const token = /\/([^\s/@#<>]+)/g;

  for (const node of nodes) {
    const text = node.data;
    if (!text.includes("/")) continue;
    // Already wrapped tokens are skipped, which keeps a re-run idempotent.
    if (node.parentElement?.closest("pre, code, a, .katex, button, .file-mention-token")) continue;

    const fragment = document.createDocumentFragment();
    let cursor = 0;
    let found = false;
    token.lastIndex = 0;
    let match: RegExpExecArray | null;
    while ((match = token.exec(text)) !== null) {
      if (match.index > 0 && !/\s/.test(text[match.index - 1])) continue;
      // Punctuation belongs to the sentence, not to the skill name.
      const raw = match[0].replace(/[.,;:!?)\]}"']+$/, "");
      if (!known.has(raw.slice(1))) continue;
      found = true;
      if (match.index > cursor) {
        fragment.appendChild(document.createTextNode(text.slice(cursor, match.index)));
      }
      const span = document.createElement("span");
      span.className = "file-mention-token";
      span.textContent = raw;
      fragment.appendChild(span);
      cursor = match.index + raw.length;
    }
    if (!found) continue;
    if (cursor < text.length) {
      fragment.appendChild(document.createTextNode(text.slice(cursor)));
    }
    node.replaceWith(fragment);
  }
}

/**
 * Wrap `+agent` mentions in the body with the same accent span `@file` and
 * `/skill` mentions wear. Only a token naming a known agent is highlighted:
 * `+` is an ordinary character in arithmetic (`2 + 2`) and in `C++`, so an
 * unknown token must stay exactly as typed.
 */
function highlightAgentMentions(root: HTMLElement, known: Set<string>) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  while (walker.nextNode()) nodes.push(walker.currentNode as Text);

  const token = /\+([^\s+@/<>#]+)/g;

  for (const node of nodes) {
    const text = node.data;
    if (!text.includes("+")) continue;
    // Already wrapped tokens are skipped, which keeps a re-run idempotent.
    if (node.parentElement?.closest("pre, code, a, .katex, button, .file-mention-token")) continue;

    const fragment = document.createDocumentFragment();
    let cursor = 0;
    let found = false;
    token.lastIndex = 0;
    let match: RegExpExecArray | null;
    while ((match = token.exec(text)) !== null) {
      // Only a token that opens the line or follows whitespace is a mention.
      if (match.index > 0 && !/\s/.test(text[match.index - 1])) continue;
      // Punctuation belongs to the sentence, not to the agent token.
      const raw = match[0].replace(/[.,;:!?，。；：！？)\]}"']+$/, "");
      const name = raw.slice(1);
      if (!known.has(name)) continue;
      found = true;
      if (match.index > cursor) {
        fragment.appendChild(document.createTextNode(text.slice(cursor, match.index)));
      }
      const span = document.createElement("span");
      span.className = "file-mention-token";
      span.textContent = raw;
      fragment.appendChild(span);
      cursor = match.index + raw.length;
    }
    if (!found) continue;
    if (cursor < text.length) {
      fragment.appendChild(document.createTextNode(text.slice(cursor)));
    }
    node.replaceWith(fragment);
  }
}

function extractEmbeddedThoughtProcess(content: string): {
  reasoningContent: string;
  mainContent: string;
} {
  const detailsPrefix = "<details><summary>Thought Process</summary>";
  const detailsSuffix = "</details>";

  const trimmed = content.trimStart();
  if (!trimmed.startsWith(detailsPrefix)) {
    return { reasoningContent: "", mainContent: content };
  }

  const endIdx = trimmed.indexOf(detailsSuffix);
  if (endIdx < 0) {
    return { reasoningContent: "", mainContent: content };
  }

  const reasoningContent = trimmed
    .slice(detailsPrefix.length, endIdx)
    .trim();
  const mainContent = trimmed
    .slice(endIdx + detailsSuffix.length)
    .trimStart();

  return { reasoningContent, mainContent };
}

/** Clock label for a mid-turn injection ("14:03"), local time. */
function formatInjectionTime(createdAtMs: number): string {
  if (!createdAtMs) return "";
  return new Date(createdAtMs).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
}

/**
 * One mid-turn injection as it reads in the transcript.
 *
 * A note longer than three lines folds: a pasted paragraph otherwise pushes the
 * reply it belongs to out of view, and one turn is meant to stay scannable.
 * Whether the text really overflows is *measured* after paint rather than
 * counted from `\n` — a single long line wraps past three rows while a two-line
 * note never reaches the limit — so a note that fits gets no dead "show more"
 * action.
 */
function InjectedNote({ injection }: { injection: ChatInjection }) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const [overflows, setOverflows] = useState(false);
  const textRef = useRef<HTMLSpanElement>(null);

  // Skipped while open: the unfolded box is the reference for the next
  // measurement, so collapsing re-reads the clamped height.
  useLayoutEffect(() => {
    const el = textRef.current;
    if (!el || open) return;
    setOverflows(el.scrollHeight - el.clientHeight > 1);
  }, [injection.text, open]);

  return (
    <div className="message-injection">
      <span className="message-injection-head">
        <FontAwesomeIcon icon={faPenToSquare} aria-hidden="true" />
        {t("chat.injected")}
        <span className="message-injection-time">
          {formatInjectionTime(injection.created_at_ms)}
        </span>
      </span>
      <span
        ref={textRef}
        className={`message-injection-text${open ? "" : " is-folded"}`}
      >
        {injection.text}
      </span>
      {overflows && (
        <button
          type="button"
          className="inline-edit-btn message-injection-toggle"
          aria-expanded={open}
          onClick={() => setOpen((previous) => !previous)}
        >
          {t(open ? "chat.foldLess" : "chat.foldMore")}
        </button>
      )}
    </div>
  );
}

/**
 * Upper bound on the text the streaming typewriter animates per commit. A
 * mid-stream markdown rewrite (an opening `**` turning into real bold) changes
 * a whole paragraph; without the cap that paragraph would re-fade at once.
 */
const STREAM_ANIMATED_CHARS = 400;

/**
 * How often the streamed markdown is parsed and committed. Unlike the old
 * typewriter this is not tied to the fade length: each character carries its
 * own start time on a timeline that survives the next `innerHTML` rebuild, so
 * a commit only decides how often new text is picked up.
 */
const STREAM_FLUSH_MS = 110;

/** Fade length per character. Mirrors `--t-dur-stream` in App.css. */
const STREAM_CHAR_FADE_MS = 200;

/**
 * Longest spread between the first and last character of one commit. Kept
 * below `STREAM_FLUSH_MS` so the next commit always starts after every
 * character of this one has started — reading order never inverts.
 */
const STREAM_CHAR_SPREAD_MS = 90;

/** Per-character delay cap; larger commits compress to stay inside the spread. */
const STREAM_CHAR_STAGGER_MS = 14;

/**
 * Upper bound on the spans built per commit. A burst that delivers a whole
 * paragraph shares one span between adjacent characters instead of creating
 * hundreds of animated elements.
 */
const STREAM_CHAR_SPAN_LIMIT = 120;

/**
 * Throttle a streaming reply to one markdown commit per `STREAM_FLUSH_MS`.
 * State is updated from an interval that always reads the latest prop, so the
 * timer never restarts as tokens arrive. When the turn ends the full text is
 * returned immediately instead of waiting for the next tick.
 */
function useStreamedText(text: string, streaming: boolean): string {
  const [shown, setShown] = useState(text);
  const latest = useRef(text);
  latest.current = text;

  useEffect(() => {
    if (!streaming) {
      setShown(latest.current);
      return;
    }
    const timer = window.setInterval(() => {
      setShown((previous) => (latest.current === previous ? previous : latest.current));
    }, STREAM_FLUSH_MS);
    return () => window.clearInterval(timer);
  }, [streaming]);

  return streaming ? shown : text;
}

/** One loop of the writing indicator's nib + ink animations, in ms. */
const WRITING_LOOP_MS = 1500;

/**
 * Builds the inline "assistant is writing" indicator: a pen nib scribbling
 * over a self-drawing ink stroke. Markdown arrives as raw HTML, so the
 * indicator is created imperatively and appended to the last rendered block —
 * the only way to place it exactly at the end of the typewriter text.
 */
function createWritingIndicator(): HTMLSpanElement {
  const svgNs = "http://www.w3.org/2000/svg";
  const indicator = document.createElement("span");
  indicator.className = "message-writing";
  indicator.setAttribute("aria-hidden", "true");

  const nib = document.createElement("span");
  nib.className = "message-writing-nib";
  const [iconWidth, iconHeight, , , iconPathData] = faPen.icon;
  const nibSvg = document.createElementNS(svgNs, "svg");
  nibSvg.setAttribute("viewBox", `0 0 ${iconWidth} ${iconHeight}`);
  nibSvg.setAttribute("aria-hidden", "true");
  nibSvg.setAttribute("focusable", "false");
  const nibPath = document.createElementNS(svgNs, "path");
  nibPath.setAttribute("fill", "currentColor");
  nibPath.setAttribute("d", Array.isArray(iconPathData) ? iconPathData.join(" ") : iconPathData);
  nibSvg.appendChild(nibPath);
  nib.appendChild(nibSvg);

  const ink = document.createElementNS(svgNs, "svg");
  ink.setAttribute("class", "message-writing-ink");
  ink.setAttribute("viewBox", "0 0 18 5");
  ink.setAttribute("aria-hidden", "true");
  const inkPath = document.createElementNS(svgNs, "path");
  inkPath.setAttribute("pathLength", "100");
  inkPath.setAttribute("d", "M1 3.4C2.5 1.2 4 4.2 5.6 2.6S8.4 1.4 10 3.2 13.2 4.4 15 2.2 16.6 1.8 17 2.4");
  ink.appendChild(inkPath);

  indicator.append(nib, ink);
  return indicator;
}

/** Block elements the writing indicator may sit inside (deepest wins). */
const WRITING_TAIL_BLOCKS = new Set([
  "P", "LI", "UL", "OL", "BLOCKQUOTE", "H1", "H2", "H3", "H4", "H5", "H6", "DIV", "DETAILS",
]);

/**
 * Deepest block at the end of the rendered markdown that can host an inline
 * indicator (last paragraph, list item or heading). Code, formulas and tables
 * keep their own layout, so the indicator follows them instead of entering.
 */
function writingTailHost(content: HTMLElement): HTMLElement {
  let host = content;
  let child = host.lastElementChild as HTMLElement | null;
  while (child) {
    if (
      !WRITING_TAIL_BLOCKS.has(child.tagName) ||
      child.matches("pre, .code-block-wrapper, .katex-display, table")
    ) {
      break;
    }
    host = child;
    child = host.lastElementChild as HTMLElement | null;
  }
  return host;
}

export function ChatMessage({ message, showRetry = false, onRetry, onDelete, onFork, dbId, onConfirm, onCancelBackground, backgroundTasks, backgroundWaiting, skillNames, agentTokens, sessionId, todoActive }: Props) {
  const { t } = useI18n();
  const isUser = message.role === "user";
  const isAssistant = message.role === "assistant";
  const [showActions, setShowActions] = useState(true);

  // Display text: collapse multimodal parts to their textual portion.
  const textContent = contentToText(message.content);
  // An attachment is named, never listed: every file is already called by the
  // accent `@name` token in the body (`highlightFileMentions`), so a chip row
  // underneath only repeats it — and repeated the machine path before the
  // mention was renamed. An image is the one exception: it *is* the content,
  // so it stays visible as a thumbnail.
  const imageAttachments = isUser
    ? (message.attachments ?? []).filter((a) => a.kind === "image" && Boolean(a.data_url))
    : [];
  // Background tool results reach the model through the injection list, but
  // they are not conversation: a turn shows only what was actually said, so
  // they are filtered out of the transcript here.
  const visibleInjections = (message.injections ?? []).filter(
    (injection) => injection.kind !== "background_result",
  );
  const displayContent = isUser
    ? textContent.replace(/<details><summary>Attached File:[^<]*<\/summary>[\s\S]*?<\/details>/g, "").trim()
    : textContent;
  const embeddedThought = extractEmbeddedThoughtProcess(displayContent);
  const reasoningContent = message.reasoning_content ?? embeddedThought.reasoningContent;
  const mainContent = message.reasoning_content
    ? displayContent
    : embeddedThought.mainContent;
  const renderedReasoningContent = reasoningContent
    ? renderMarkdown(md, reasoningContent)
    : "";
  // The reply body is throttled while it streams: see `useStreamedText`.
  const streamedMainContent = useStreamedText(
    mainContent,
    isAssistant && Boolean(message.streaming),
  );
  // Raw HTML stays off, so a figure is cut out of the reply *before* markdown-it
  // runs and leaves a slot token behind (`extractFigures`); `mountFigures`
  // turns each token into a real, sanitised card once the subtree is written.
  // Only a reply draws: what the user typed stays literal, so a pasted `svg`
  // snippet in the composer is still code.
  const replyStreaming = isAssistant && Boolean(message.streaming);
  const rendered = useMemo<{ html: string; figures: FigureSpec[]; diagrams: DiagramSpec[] }>(() => {
    // The user's own text is rendered by the literal profile: no image rule, so
    // a typed `![a](b)` stays those characters instead of becoming an element.
    if (!isAssistant) {
      return { html: renderMarkdown(mdLiteral, streamedMainContent), figures: [], diagrams: [] };
    }
    const extracted = extractFigures(streamedMainContent, { streaming: replyStreaming });
    // Mermaid runs second: an ordinary fence survives `extractFigures` verbatim,
    // so both kinds of drawing can share one reply.
    const diagrams = extractDiagrams(extracted.markdown, { streaming: replyStreaming });
    return {
      html: renderMarkdown(md, diagrams.markdown),
      figures: extracted.figures,
      diagrams: diagrams.diagrams,
    };
  }, [streamedMainContent, replyStreaming, isAssistant]);
  const renderedMainContent = rendered.html;
  const figures = rendered.figures;
  const diagrams = rendered.diagrams;
  const [reasoningOpen, setReasoningOpen] = useState(false);

  // Collapsed-state live preview: the last three reasoning lines are rendered
  // into a 2-row viewport over a 3-row track. When a new line arrives the
  // track re-mounts (keyed by line count) and replays the scroll-up tick:
  // the oldest line scrolls out of the viewport while fading away, the other
  // two step up one row.
  const reasoningPreview = useMemo(() => {
    const lines = reasoningContent
      .split(/\r?\n/)
      .map((l) => l.trim())
      .filter((l) => l.length > 0);
    return {
      older: lines[lines.length - 3] ?? "",
      previous: lines[lines.length - 2] ?? "",
      current: lines[lines.length - 1] ?? "",
      lineCount: lines.length,
    };
  }, [reasoningContent]);

  // Live reply rendering happens outside React: the markdown subtree is
  // written imperatively only when the rendered HTML changes, and the pen is
  // re-attached after every commit (the parent re-renders on every token,
  // which would otherwise wipe the imperatively added indicator). The text
  // diff below is a no-op when nothing visibly changed.
  //
  // Deliberately no dependency array: the check runs after every commit so the
  // pen is back before paint, even when the HTML itself did not change.
  const contentRef = useRef<HTMLDivElement>(null);
  const streamedTextRef = useRef("");
  const renderedHtmlRef = useRef("");
  /** The streaming flag the mounted cards were built with (see the rebuild below). */
  const mountedStreamingRef = useRef(false);
  const appliedSkillNamesRef = useRef("");
  const appliedAgentTokensRef = useRef("");
  const writingPenRef = useRef<HTMLSpanElement | null>(null);
  const writingStartRef = useRef(0);
  // Per-character reveal schedule. `starts[i]` is the absolute time at which
  // character `base + i` of the rendered text begins to fade in. It outlives
  // the innerHTML rebuild: a character still fading when the next commit
  // replaces the subtree is re-wrapped with a negative animation delay, which
  // resumes it at the frame it had reached instead of snapping to full opacity.
  const streamRevealRef = useRef<{ base: number; starts: number[] }>({
    base: 0,
    starts: [],
  });
  useLayoutEffect(() => {
    const el = contentRef.current;
    if (!el) return;

    // The markdown subtree is owned imperatively (the JSX renders this div
    // empty), so React never touches it. Tokens still arrive faster than the
    // visible flush, and each rebuild destroys the reveal spans mid-fade, so
    // the HTML is replaced only when it actually changed; the schedule below
    // re-wraps whatever was still fading.
    const htmlChanged = renderedHtmlRef.current !== renderedMainContent;
    // A reply that just stopped streaming is rebuilt once more even when its
    // text did not change: a dynamic figure is a frame element, Chromium reloads
    // a re-inserted frame, so it waits for the commit nothing follows instead of
    // restarting on every token. Its slots only exist in a freshly written tree.
    const streamingChanged = mountedStreamingRef.current !== replyStreaming;
    if (htmlChanged || streamingChanged) {
      el.innerHTML = renderedMainContent;
      renderedHtmlRef.current = renderedMainContent;
      mountedStreamingRef.current = replyStreaming;
      // A figure card is cached by its source hash, so the rebuild a streamed
      // token causes re-inserts the same canvas — the orbit angle a user just
      // dragged is not reset on every token. A card that fails to mount (a
      // three.js chunk that did not load, a malformed spec) must not cost the
      // whole message: the text is already painted, so the slot stays empty.
      try {
        mountFigures(el, figures, {
          messageId: String(message.id),
          streaming: replyStreaming,
        });
      } catch (error) {
        console.error("Figure card failed to mount", error);
      }
      // Mermaid renders asynchronously, so the card is placed here and paints
      // itself when its chunk has loaded and the SVG is ready.
      try {
        mountDiagrams(el, diagrams, { messageId: String(message.id) });
      } catch (error) {
        console.error("Diagram card failed to mount", error);
      }
      // The user's body never changes, so this runs once per turn.
      if (isUser) {
        try {
          highlightFileMentions(el);
        } catch (error) {
          console.error("Mention highlighting failed", error);
        }
        // A freshly painted body carries no `/skill` or `+agent` spans yet.
        appliedSkillNamesRef.current = "";
        appliedAgentTokensRef.current = "";
      }
    }

    // Skill names arrive with the `/skill` popup's first load, which can happen
    // after a restored history was already painted — re-run the highlight then.
    // Re-running is safe: `highlightSkillMentions` skips the spans it wrapped.
    if (isUser && skillNames && skillNames.length > 0) {
      const key = skillNames.join("\n");
      if (key !== appliedSkillNamesRef.current) {
        highlightSkillMentions(el, new Set(skillNames));
        appliedSkillNamesRef.current = key;
      }
    }

    // The `+agent` roster is preloaded at startup, so a restored transcript can
    // be highlighted as soon as it is painted; the key guard keeps this cheap
    // on every later commit.
    if (isUser && agentTokens && agentTokens.length > 0) {
      const key = agentTokens.join("\n");
      if (key !== appliedAgentTokensRef.current) {
        highlightAgentMentions(el, new Set(agentTokens));
        appliedAgentTokensRef.current = key;
      }
    }

    // Pen: visible for the whole live turn, pinned to the typewriter's tail.
    if (!isAssistant || !message.streaming) {
      writingPenRef.current?.remove();
      writingPenRef.current = null;
      writingStartRef.current = 0;
    } else {
      let pen = writingPenRef.current;
      if (!pen) {
        pen = createWritingIndicator();
        writingPenRef.current = pen;
        writingStartRef.current = performance.now();
      }
      const host = writingTailHost(el);
      if (pen.parentNode !== host) host.appendChild(pen);
      // The pen is re-attached after every innerHTML rebuild; a negative delay
      // keeps the nib/ink loop where it was instead of restarting at zero.
      const phase = (performance.now() - writingStartRef.current) % WRITING_LOOP_MS;
      pen.style.setProperty("--writing-delay", `-${Math.round(phase)}ms`);
    }

    const current = el.textContent ?? "";
    const previous = streamedTextRef.current;
    streamedTextRef.current = current;

    if (!message.streaming) {
      streamRevealRef.current.base = 0;
      streamRevealRef.current.starts.length = 0;
      return;
    }
    if (!htmlChanged || current.length === 0) return;

    // Longest shared prefix and suffix. The prefix is the text that only grew
    // (usually an append); the suffix matters when markdown syntax closes and
    // removes characters from the middle (`**bold**` losing its asterisks) —
    // everything after that marker is unchanged and must not fade again.
    let prefix = 0;
    const shared = Math.min(previous.length, current.length);
    while (prefix < shared && previous.charCodeAt(prefix) === current.charCodeAt(prefix)) {
      prefix += 1;
    }
    // Cap a large rewrite so it never re-fades a whole long reply at once.
    prefix = Math.min(previous.length, Math.max(prefix, current.length - STREAM_ANIMATED_CHARS));
    if (prefix > 0) {
      // Never start a run in the middle of a surrogate pair (emoji).
      const before = current.charCodeAt(prefix - 1);
      const after = current.charCodeAt(prefix);
      if (before >= 0xd800 && before <= 0xdbff && after >= 0xdc00 && after <= 0xdfff) {
        prefix -= 1;
      }
    }

    let suffix = 0;
    const suffixLimit = Math.min(previous.length, current.length) - prefix;
    while (
      suffix < suffixLimit &&
      previous.charCodeAt(previous.length - 1 - suffix) ===
        current.charCodeAt(current.length - 1 - suffix)
    ) {
      suffix += 1;
    }
    const suffixStart = current.length - suffix;

    const now = performance.now();
    const schedule = streamRevealRef.current;
    const oldBase = schedule.base;
    const oldStarts = schedule.starts;
    const visibleAt = now - STREAM_CHAR_FADE_MS - 1;

    // Only the tail is animated; older text is already visible and never needs
    // a wrapper. The window also bounds the schedule array's size.
    let windowStart = Math.max(0, current.length - STREAM_ANIMATED_CHARS);
    if (windowStart > 0) {
      // Never begin the window in the middle of a surrogate pair (emoji).
      const before = current.charCodeAt(windowStart - 1);
      const after = current.charCodeAt(windowStart);
      if (before >= 0xd800 && before <= 0xdbff && after >= 0xdc00 && after <= 0xdfff) {
        windowStart -= 1;
      }
    }
    const starts = new Array<number>(current.length - windowStart).fill(visibleAt);

    // Shared prefix: keep every start that is still inside the window.
    for (let index = windowStart; index < prefix; index += 1) {
      const oldOffset = index - oldBase;
      if (oldOffset >= 0 && oldOffset < oldStarts.length) {
        starts[index - windowStart] = oldStarts[oldOffset];
      }
    }

    // Shared suffix: characters that already existed keep their original
    // start (or stay visible), even though the middle before them changed.
    const suffixStartOld = previous.length - suffix;
    for (let index = Math.max(suffixStart, windowStart); index < current.length; index += 1) {
      const oldOffset = suffixStartOld + (index - suffixStart) - oldBase;
      if (oldOffset >= 0 && oldOffset < oldStarts.length) {
        starts[index - windowStart] = oldStarts[oldOffset];
      }
    }

    // The rewritten/new middle gets fresh starts. A small commit gets the full
    // stagger (a visible character-by-character cascade); a burst compresses
    // its delay and groups adjacent characters so it still lands inside the
    // spread.
    const middleFrom = Math.max(prefix, windowStart);
    const middleCount = Math.max(0, suffixStart - middleFrom);
    let groupSize = 1;
    if (middleCount > 0) {
      groupSize = Math.max(1, Math.ceil(middleCount / STREAM_CHAR_SPAN_LIMIT));
      const groups = Math.ceil(middleCount / groupSize);
      const step =
        groups > 1
          ? Math.min(STREAM_CHAR_STAGGER_MS, STREAM_CHAR_SPREAD_MS / (groups - 1))
          : 0;
      let offset = 0;
      let group = 0;
      while (offset < middleCount) {
        let end = Math.min(offset + groupSize, middleCount);
        if (end < middleCount) {
          // A run must not end between a high and a low surrogate.
          const before = current.charCodeAt(middleFrom + end - 1);
          const after = current.charCodeAt(middleFrom + end);
          if (before >= 0xd800 && before <= 0xdbff && after >= 0xdc00 && after <= 0xdfff) {
            end += 1;
          }
        }
        if (end <= offset) end = Math.min(offset + 2, middleCount);
        const startedAt = now + group * step;
        for (let index = offset; index < end; index += 1) {
          starts[middleFrom - windowStart + index] = startedAt;
        }
        offset = end;
        group += 1;
      }
    }

    // Forget characters whose fade has already finished; they are plain visible
    // text now and never need a wrapper again.
    const firstActive = starts.findIndex(
      (startedAt) => startedAt + STREAM_CHAR_FADE_MS > now,
    );
    if (firstActive === -1) {
      schedule.base = current.length;
      schedule.starts.length = 0;
      return;
    }
    schedule.base = windowStart + firstActive;
    schedule.starts = starts.slice(firstActive);

    let activeFrom = schedule.base;
    if (activeFrom > 0 && activeFrom < current.length) {
      // Do not let the split point fall between a high and a low surrogate.
      const before = current.charCodeAt(activeFrom - 1);
      const after = current.charCodeAt(activeFrom);
      if (before >= 0xd800 && before <= 0xdbff && after >= 0xdc00 && after <= 0xdfff) {
        schedule.base = activeFrom - 1;
        schedule.starts.unshift(schedule.starts[0]);
        activeFrom = schedule.base;
      }
    }
    if (activeFrom >= current.length) return;

    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    const textNodes: Array<{ node: Text; start: number }> = [];
    let offset = 0;
    while (walker.nextNode()) {
      const node = walker.currentNode as Text;
      textNodes.push({ node, start: offset });
      offset += node.data.length;
    }

    for (const { node, start } of textNodes) {
      if (start + node.data.length <= activeFrom) continue;
      if (!node.data.trim()) continue;
      // Code, formulas and controls already carry their own markup; a wrapper
      // there would fight highlight.js / KaTeX or animate a button label.
      if (node.parentElement?.closest("pre, svg, math, annotation, button, a, .katex, .chat-figure")) continue;

      let tail = node;
      let tailStart = start;
      if (start < activeFrom) {
        tail = node.splitText(activeFrom - start);
        tailStart = activeFrom;
      }
      const parent = tail.parentNode;
      if (!parent) continue;

      const text = tail.data;
      const fragment = document.createDocumentFragment();
      let local = 0;
      while (local < text.length) {
        let end = Math.min(local + groupSize, text.length);
        // Extend a run that would otherwise end between a high and a low
        // surrogate, so an emoji never fades in two pieces.
        if (end < text.length) {
          const before = text.charCodeAt(end - 1);
          const after = text.charCodeAt(end);
          if (before >= 0xd800 && before <= 0xdbff && after >= 0xdc00 && after <= 0xdfff) {
            end += 1;
          }
        }
        if (end <= local) end = Math.min(local + 2, text.length);

        const startedAt = schedule.starts[tailStart + local - schedule.base];
        const char = document.createElement("span");
        char.className = "stream-char";
        // Negative delays resume a fade that the previous DOM had already
        // started; positive ones hold a character back until its turn.
        char.style.animationDelay = `${Math.round(startedAt - now)}ms`;
        char.textContent = text.slice(local, end);
        fragment.appendChild(char);
        local = end;
      }
      parent.replaceChild(fragment, tail);
    }
  });

  return (
    <div
      className={`chat-message-shell ${isUser ? "user" : "assistant"}`}
      onMouseEnter={() => setShowActions(true)}
    // onMouseLeave={() => setShowActions(false)}
    >
      <div className={`chat-message ${isUser ? "user" : "assistant"}`}>
        {/* <div className="message-role">{isUser ? "You" : "Assistant"}</div> */}

        {reasoningContent && (
          <details
            className="message-reasoning"
            open={reasoningOpen}
            onToggle={(e) => setReasoningOpen((e.target as HTMLDetailsElement).open)}
          >
            <summary className="message-reasoning-summary">
              <span className="message-reasoning-head">
                <span className="message-reasoning-title">{t("chat.thoughtProcess")}</span>
              </span>
              {!reasoningOpen && reasoningPreview.current && (
                <span
                  className={`reasoning-preview ${message.streaming ? "reasoning-preview-live" : ""}`}
                  aria-hidden="true"
                >
                  <span
                    className="reasoning-preview-track"
                    key={reasoningPreview.lineCount}
                  >
                    <span className="reasoning-preview-line reasoning-preview-old">
                      {reasoningPreview.older || "\u00A0"}
                    </span>
                    <span className="reasoning-preview-line reasoning-preview-prev">
                      {reasoningPreview.previous || "\u00A0"}
                    </span>
                    <span className="reasoning-preview-line reasoning-preview-cur">
                      {reasoningPreview.current}
                    </span>
                  </span>
                </span>
              )}
            </summary>
            <div
              className="message-reasoning-content"
              dangerouslySetInnerHTML={{ __html: renderedReasoningContent }}
            />
          </details>
        )}
        {isAssistant && <ToolCallGroup message={message} onConfirm={onConfirm} onCancelBackground={onCancelBackground} />}
        {isAssistant && <TodoList sessionId={sessionId} active={todoActive} />}
        {isAssistant && backgroundTasks !== undefined && (
          <BackgroundTaskGroup
            tasks={backgroundTasks}
            waiting={backgroundWaiting ?? 0}
            onCancelTask={onCancelBackground}
          />
        )}
        <div
          className="message-content"
          ref={contentRef}
          onClick={(e) => {
            const target = e.target as HTMLElement;
            const copyButton = target.closest(".code-copy-btn") as HTMLButtonElement | null;
            if (copyButton) {
              const wrapper = copyButton.closest(".code-block-wrapper");
              const code = wrapper?.querySelector("pre code");
              const text = code?.textContent ?? "";
              if (text) {
                tauriWriteText(text).catch(() => {
                  // Fallback: standard Web API
                  navigator.clipboard.writeText(text).catch(() => {
                    // Fallback: legacy execCommand
                    const textarea = document.createElement("textarea");
                    textarea.value = text;
                    document.body.appendChild(textarea);
                    textarea.select();
                    document.execCommand("copy");
                    document.body.removeChild(textarea);
                  });
                });
                const originalText = copyButton.textContent;
                copyButton.textContent = t("common.copied");
                window.setTimeout(() => {
                  copyButton.textContent = originalText;
                }, 1200);
              }
              return;
            }
            const link = target.closest("a");
            if (link) {
              e.preventDefault();
            }
          }}
        />
        {/* Information the user pushed into this turn while it was running.
            One turn stays one bubble, so the block sits under the reply and
            states when the model actually received it. Background task
            results are replayed into the model history but stay out of the
            transcript (see `visibleInjections`). */}
        {isAssistant && visibleInjections.length > 0 && (
          <div className="message-injections">
            {visibleInjections.map((injection) => (
              <InjectedNote key={injection.id} injection={injection} />
            ))}
          </div>
        )}
        {imageAttachments.length > 0 && (
          <div className="message-attachments">
            {imageAttachments.map((att, i) => (
              <a
                key={`${att.name}-${i}`}
                className="message-attachment-thumb"
                href={att.data_url}
                target="_blank"
                rel="noopener noreferrer"
                title={mentionDisplayName(att.name)}
              >
                <img src={att.data_url} alt={mentionDisplayName(att.name)} />
              </a>
            ))}
          </div>
        )}

      </div>

      {/* Message action buttons (delete / fork / export).
          The user's retry lives in the same outside-the-bubble row, aligned
          to its own edge — the card stays a pure content block. A failed turn
          offers no export: there is no finished reply to hand out, and with
          the failure row gone that icon would be all that is left at the
          bottom of the bubble. */}
      {isAssistant && showActions && !message.streaming && (
        <div className="message-action-buttons">
          {isAssistant && !message.error && (
            <button
              type="button"
              className="message-action-btn export"
              onClick={() => {
                // Building the document may re-render diagrams, so it is async.
                void printableMessageHtml(contentRef.current, renderedMainContent)
                  .then((html) => exportAssistantMessagePdf(html, message.id))
                  .catch((error) => console.error("Message export failed", error));
              }}
              disabled={!mainContent.trim()}
              title={t("chat.exportTitle")}
            >
              <FontAwesomeIcon icon={faFileArrowDown} />
            </button>
          )}
          {figures.some((figure) => !figure.pending) && (
            <button
              type="button"
              className="message-action-btn save-figures"
              onClick={() => {
                const root = contentRef.current;
                if (root) void saveAllFigures(root);
              }}
              title={t("figure.saveAll")}
            >
              <FontAwesomeIcon icon={faImages} />
            </button>
          )}
          {onDelete && dbId !== undefined && (
            <button
              type="button"
              className="message-action-btn delete"
              onClick={() => onDelete(message.id)}
              title={t("chat.deleteTitle")}
            >
              <FontAwesomeIcon icon={faTrash} />
            </button>
          )}
          {onFork && dbId !== undefined && (
            <button
              type="button"
              className="message-action-btn fork"
              onClick={() => onFork(message.id)}
              title={t("chat.forkTitle")}
            >
              <FontAwesomeIcon icon={faCodeBranch} />
            </button>
          )}
        </div>
      )}
      {isUser && showRetry && (
        <div className="message-action-buttons">
          <button
            type="button"
            className="message-action-btn"
            onClick={onRetry}
            title={t("chat.retryTitle")}
          >
            <FontAwesomeIcon icon={faRotateRight} />
            {t("common.retry")}
          </button>
        </div>
      )}
    </div>
  );
}

/**
 * Keeps one broken message from taking the transcript down with it.
 *
 * The markdown pipeline already degrades to literal text when a renderer
 * throws, but an error raised *while React renders* this component (a malformed
 * figure spec, a plugin that throws outside `render`) would otherwise unmount
 * the whole tree — the window goes blank and the console shows the stack. The
 * boundary turns that into one readable row.
 */
export class ChatMessageBoundary extends Component<
  { children: ReactNode },
  { error: string | null }
> {
  state: { error: string | null } = { error: null };

  static getDerivedStateFromError(error: unknown): { error: string } {
    return { error: error instanceof Error ? error.message : String(error) };
  }

  componentDidCatch(error: unknown): void {
    console.error("Message rendering failed", error);
  }

  render(): ReactNode {
    if (this.state.error === null) return this.props.children;
    return (
      <div className="chat-message-shell assistant">
        <div className="chat-message assistant">
          <div className="message-content">
            <pre className="markdown-fallback">
              {text("app.messageRenderFailed", { error: this.state.error })}
            </pre>
          </div>
        </div>
      </div>
    );
  }
}
