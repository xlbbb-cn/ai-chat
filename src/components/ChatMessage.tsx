import type { BackgroundTaskState, ChatInjection, Message, MessageContent } from "../types";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
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
// `t` (module-level) is used by the markdown-it renderer rules, which run
// outside the React tree; the hook keeps the component re-rendering on change.
import { t as text, useI18n } from "../i18n";
import {
  FontAwesomeIcon,
  faCodeBranch,
  faFileArrowDown,
  faFileLines,
  faPaperclip,
  faPen,
  faPenToSquare,
  faRotateRight,
  faTrash,
  faVolumeHigh,
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

const md = new MarkdownIt({
  // Disable raw HTML and auto-linking to prevent navigation vectors.
  html: false,
  linkify: false,
  typographer: true,
  breaks: true,
});

md.renderer.rules.fence = (tokens, idx) => {
  const token = tokens[idx];
  const info = token.info ? token.info.trim().split(/\s+/g)[0] : "";
  const language = info && hljs.getLanguage(info) ? info : "";
  const code = escapeHtml(token.content);
  const highlighted = language
    ? hljs.highlight(token.content, { language }).value
    : code;
  const className = language ? `language-${md.utils.escapeHtml(language)}` : "";

  return `
    <div class="code-block-wrapper">
      <div class="code-block-toolbar">
        <button type="button" class="code-copy-btn">${escapeHtml(text("common.copy"))}</button>
      </div>
      <pre class="hljs"><code class="${className}">${highlighted}</code></pre>
    </div>
  `;
};

// Remove markdown link parsing entirely ([text](url), autolink) and strip any fallback anchor tokens.
md.disable(["link", "autolink"]);
md.renderer.rules.link_open = () => "<span>";
md.renderer.rules.link_close = () => "</span>";

// Render `$...$` / `$$...$$` formulas with KaTeX.
md.use(katexMathPlugin);

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
    </style>
  </head>
  <body>
    <main>${contentHtml}</main>
  </body>
</html>`;
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
  /** Session this message belongs to — the todo panel fetches its active list. */
  sessionId?: string;
  /**
   * Mirror the session's todo list under this message. Only the newest
   * assistant message gets it, so the list is drawn once per session.
   */
  todoActive?: boolean;
}

function extractAttachmentNames(content: string): string[] {
  const names: string[] = [];
  const re = /<details><summary>Attached File: ([^<]+)<\/summary>/g;
  let m;
  while ((m = re.exec(content)) !== null) {
    names.push(m[1].trim());
  }
  return names;
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

  // Mirrors `parseMentions` in `fileMention.ts`: a path, then an optional
  // `#12` / `#12-24` / `#12-` line range.
  const token = /@[^\s@#<>]+(?:#\d+(?:-\d*)?)?/g;

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
const STREAM_INK_MAX_CHARS = 400;

/**
 * Streaming arrives token by token, so `dangerouslySetInnerHTML` would be
 * rewritten dozens of times per second. Every rewrite replaces the whole
 * subtree — including the `.stream-ink` wrapper that is still fading — so the
 * new text never actually fades in, it pops. Rendering in ~4 commits/s gives
 * each chunk the full fade window before the next rebuild replaces it, while
 * the visible delay stays below the model's own token cadence.
 */
const STREAM_FLUSH_MS = 220;

/**
 * Throttle a streaming reply to one visible update per `STREAM_FLUSH_MS`.
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

export function ChatMessage({ message, showRetry = false, onRetry, onDelete, onFork, dbId, onConfirm, onCancelBackground, backgroundTasks, backgroundWaiting, skillNames, sessionId, todoActive }: Props) {
  const { t } = useI18n();
  const isUser = message.role === "user";
  const isAssistant = message.role === "assistant";
  const [showActions, setShowActions] = useState(true);

  // Display text: collapse multimodal parts to their textual portion.
  const textContent = contentToText(message.content);
  // `@file` mentions carry their own accent-highlighted token in the body (see
  // `highlightFileMentions`), so only attachments the text does not reference —
  // legacy picker uploads — still need the compact chip list. Matching on the
  // path alone keeps a clamped range (`#5-999` → `#5-7`) from re-adding a chip.
  const attachmentNames = (
    isUser && message.attachments
      ? message.attachments.map((a) => a.name)
      : isUser
        ? extractAttachmentNames(textContent)
        : []
  ).filter((name) => !textContent.includes(`@${name.split("#")[0]}`));
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
  const renderedReasoningContent = reasoningContent ? md.render(reasoningContent) : "";
  // The reply body is throttled while it streams: see `useStreamedText`.
  const streamedMainContent = useStreamedText(
    mainContent,
    isAssistant && Boolean(message.streaming),
  );
  const renderedMainContent = useMemo(
    () => md.render(streamedMainContent),
    [streamedMainContent],
  );
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
  const appliedSkillNamesRef = useRef("");
  const writingPenRef = useRef<HTMLSpanElement | null>(null);
  const writingStartRef = useRef(0);
  useLayoutEffect(() => {
    const el = contentRef.current;
    if (!el) return;

    // `dangerouslySetInnerHTML` is re-applied on every parent render, and
    // tokens arrive several times faster than the visible flush — each write
    // would destroy the `.stream-ink` wrapper that is still fading. Writing
    // the HTML only when it actually changed keeps the streaming DOM stable
    // between flushes, so a chunk's fade can finish.
    if (renderedHtmlRef.current !== renderedMainContent) {
      el.innerHTML = renderedMainContent;
      renderedHtmlRef.current = renderedMainContent;
      // The user's body never changes, so this runs once per turn.
      if (isUser) {
        highlightFileMentions(el);
        // A freshly painted body carries no `/skill` spans yet.
        appliedSkillNamesRef.current = "";
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
    if (!message.streaming || current.length === 0 || current === previous) return;

    // Longest shared prefix: everything past it is what just arrived. The
    // prefix (not a plain `startsWith`) is what makes this work with markdown —
    // every rendered block ends with a newline, so appending inside a paragraph
    // moves that newline to the right.
    let pivot = 0;
    const shared = Math.min(previous.length, current.length);
    while (pivot < shared && previous.charCodeAt(pivot) === current.charCodeAt(pivot)) {
      pivot += 1;
    }
    // Cap a markdown rewrite (`**` turning into real bold) so it never fades a
    // whole long reply at once.
    pivot = Math.max(pivot, current.length - STREAM_INK_MAX_CHARS);
    if (pivot >= current.length) return;

    // Never split a surrogate pair (emoji) between two text nodes.
    if (pivot > 0) {
      const before = current.charCodeAt(pivot - 1);
      const after = current.charCodeAt(pivot);
      if (before >= 0xd800 && before <= 0xdbff && after >= 0xdc00 && after <= 0xdfff) {
        pivot -= 1;
      }
    }

    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    const textNodes: Array<{ node: Text; start: number }> = [];
    let offset = 0;
    while (walker.nextNode()) {
      const node = walker.currentNode as Text;
      textNodes.push({ node, start: offset });
      offset += node.data.length;
    }

    for (const { node, start } of textNodes) {
      if (start + node.data.length <= pivot) continue;
      if (!node.data.trim()) continue;
      // Code, formulas and controls already carry their own markup; a wrapper
      // there would fight highlight.js / KaTeX or animate a button label.
      if (node.parentElement?.closest("pre, svg, math, annotation, button, a, .katex")) continue;
      const tail = start < pivot ? node.splitText(pivot - start) : node;
      const parent = tail.parentNode;
      if (!parent) continue;
      const ink = document.createElement("span");
      ink.className = "stream-ink";
      parent.insertBefore(ink, tail);
      ink.appendChild(tail);
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
        {attachmentNames.length > 0 && (
          <div className="message-attachments">
            <span>{t("chat.attachedFiles")}</span>
            {attachmentNames.map((name, i) => {
              const att = message.attachments?.find((a) => a.name === name);
              if (att?.kind === "image" && att.data_url) {
                return (
                  <a
                    key={i}
                    className="message-attachment-thumb"
                    href={att.data_url}
                    target="_blank"
                    rel="noopener noreferrer"
                    title={att.name}
                  >
                    <img src={att.data_url} alt={att.name} />
                    <span className="message-attachment-name">{att.name}</span>
                  </a>
                );
              }
              const icon =
                att?.kind === "file" ? faFileLines : att?.kind === "audio" ? faVolumeHigh : faPaperclip;
              return (
                <span key={i} className="message-attachment-pill">
                  <FontAwesomeIcon icon={icon} />
                  {name}
                </span>
              );
            })}
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
              onClick={() => exportAssistantMessagePdf(renderedMainContent, message.id)}
              disabled={!mainContent.trim()}
              title={t("chat.exportTitle")}
            >
              <FontAwesomeIcon icon={faFileArrowDown} />
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
