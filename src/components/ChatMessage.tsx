import type { Message } from "../types";
import "./ChatMessage.css";
import MarkdownIt from "markdown-it";
import hljs from "highlight.js";
import "highlight.js/styles/github.css";

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
  highlight: (str: string, lang: string): string => {
    if (lang && hljs.getLanguage(lang)) {
      try {
        return `<pre class="hljs"><code>${hljs.highlight(str, { language: lang }).value}</code></pre>`;
      } catch (__) { }
    }
    return `<pre class="hljs"><code>${escapeHtml(str)}</code></pre>`;
  },
});

// Remove markdown link parsing entirely ([text](url), autolink) and strip any fallback anchor tokens.
md.disable(["link", "autolink"]);
md.renderer.rules.link_open = () => "<span>";
md.renderer.rules.link_close = () => "</span>";

interface Props {
  message: Message;
  showRetry?: boolean;
  onRetry?: () => void;
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

export function ChatMessage({ message, showRetry = false, onRetry }: Props) {
  const isUser = message.role === "user";
  const attachmentNames = isUser ? extractAttachmentNames(message.content) : [];
  const displayContent = isUser
    ? message.content.replace(/<details><summary>Attached File:[^<]*<\/summary>[\s\S]*?<\/details>/g, "").trim()
    : message.content;
  const embeddedThought = extractEmbeddedThoughtProcess(displayContent);
  const reasoningContent = message.reasoning_content ?? embeddedThought.reasoningContent;
  const mainContent = message.reasoning_content
    ? displayContent
    : embeddedThought.mainContent;

  return (
    <div className={`chat-message ${isUser ? "user" : "assistant"}`}>
      <div className="message-role">{isUser ? "You" : "Assistant"}</div>

      {reasoningContent && (
        <details className="message-reasoning">
          <summary>Thought Process</summary>
          <div
            className="message-reasoning-content"
            dangerouslySetInnerHTML={{ __html: md.render(reasoningContent) }}
          />
        </details>
      )}
      <div
        className="message-content"
        dangerouslySetInnerHTML={{ __html: md.render(mainContent) }}
        onClick={(e) => {
          const target = e.target as HTMLElement;
          const link = target.closest("a");
          if (link) {
            e.preventDefault();
          }
        }}
      />
      {message.streaming && <span className="cursor">|</span>}
      {attachmentNames.length > 0 && (
        <div className="message-attachments">
          <span>Attached Files:</span>
          {attachmentNames.map((name, i) => (
            <span key={i} className="message-attachment-pill">📎 {name}</span>
          ))}
        </div>
      )}
      {isUser && showRetry && (
        <div className="message-actions">
          <button className="message-retry-btn" onClick={onRetry} title="Retry this unfinished user message">
            Retry
          </button>
        </div>
      )}
    </div>
  );
}
