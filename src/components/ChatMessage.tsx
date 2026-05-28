import type { Message } from "../types";
import "./ChatMessage.css";
import { useEffect, useRef, useState } from "react";
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
        <button type="button" class="code-copy-btn">Copy</button>
      </div>
      <pre class="hljs"><code class="${className}">${highlighted}</code></pre>
    </div>
  `;
};

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
  const [reasoningFlowActive, setReasoningFlowActive] = useState(false);
  const reasoningUpdateTimerRef = useRef<number | null>(null);
  const lastReasoningRef = useRef(reasoningContent);

  useEffect(() => {
    if (!message.streaming) {
      if (reasoningUpdateTimerRef.current !== null) {
        window.clearTimeout(reasoningUpdateTimerRef.current);
        reasoningUpdateTimerRef.current = null;
      }
      setReasoningFlowActive(false);
      lastReasoningRef.current = reasoningContent;
      return;
    }

    if (!reasoningContent) {
      setReasoningFlowActive(false);
      return;
    }

    if (reasoningContent !== lastReasoningRef.current) {
      lastReasoningRef.current = reasoningContent;
      setReasoningFlowActive(true);

      if (reasoningUpdateTimerRef.current !== null) {
        window.clearTimeout(reasoningUpdateTimerRef.current);
      }

      // Keep the effect visible only while updates keep arriving.
      reasoningUpdateTimerRef.current = window.setTimeout(() => {
        setReasoningFlowActive(false);
        reasoningUpdateTimerRef.current = null;
      }, 700);
    }
  }, [message.streaming, reasoningContent]);

  useEffect(() => {
    return () => {
      if (reasoningUpdateTimerRef.current !== null) {
        window.clearTimeout(reasoningUpdateTimerRef.current);
      }
    };
  }, []);

  return (
    <div className={`chat-message ${isUser ? "user" : "assistant"}`}>
      <div className="message-role">{isUser ? "You" : "Assistant"}</div>

      {reasoningContent && (
        <details className="message-reasoning">
          <summary className={`message-reasoning-summary ${reasoningFlowActive ? "reasoning-flow-active" : ""}`}>
            Thought Process
          </summary>
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
          const copyButton = target.closest(".code-copy-btn") as HTMLButtonElement | null;
          if (copyButton) {
            const wrapper = copyButton.closest(".code-block-wrapper");
            const code = wrapper?.querySelector("pre code");
            const text = code?.textContent ?? "";
            if (text) {
              if (navigator.clipboard?.writeText) {
                navigator.clipboard.writeText(text).catch(() => {
                  const textarea = document.createElement("textarea");
                  textarea.value = text;
                  document.body.appendChild(textarea);
                  textarea.select();
                  document.execCommand("copy");
                  document.body.removeChild(textarea);
                });
              } else {
                const textarea = document.createElement("textarea");
                textarea.value = text;
                document.body.appendChild(textarea);
                textarea.select();
                document.execCommand("copy");
                document.body.removeChild(textarea);
              }
              const originalText = copyButton.textContent;
              copyButton.textContent = "Copied!";
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
