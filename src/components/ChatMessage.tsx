import type { Message } from "../types";
import { useEffect, useRef, useState } from "react";
import MarkdownIt from "markdown-it";
import hljs from "highlight.js";
import "highlight.js/styles/github.css";
import "./ChatMessage.css";

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

function buildPrintableMessageHtml(contentHtml: string, exportedAt: string): string {
  return `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>${escapeHtml(exportedAt)}</title>
    <style>
      :root {
        color-scheme: light;
        font-family: "Segoe UI", "Microsoft YaHei", sans-serif;
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
        font-family: "Cascadia Code", "Consolas", monospace;
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
    window.setTimeout(() => {
      frameWindow.focus();
      frameWindow.print();
    }, 50);
  }, { once: true });

  document.body.appendChild(iframe);
}

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
  const isAssistant = message.role === "assistant";
  const attachmentNames = isUser ? extractAttachmentNames(message.content) : [];
  const displayContent = isUser
    ? message.content.replace(/<details><summary>Attached File:[^<]*<\/summary>[\s\S]*?<\/details>/g, "").trim()
    : message.content;
  const embeddedThought = extractEmbeddedThoughtProcess(displayContent);
  const reasoningContent = message.reasoning_content ?? embeddedThought.reasoningContent;
  const mainContent = message.reasoning_content
    ? displayContent
    : embeddedThought.mainContent;
  const renderedReasoningContent = reasoningContent ? md.render(reasoningContent) : "";
  const renderedMainContent = md.render(mainContent);
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
    <div className={`chat-message-shell ${isUser ? "user" : "assistant"}`}>
      <div className={`chat-message ${isUser ? "user" : "assistant"}`}>
        <div className="message-role">{isUser ? "You" : "Assistant"}</div>

        {reasoningContent && (
          <details className="message-reasoning">
            <summary className={`message-reasoning-summary ${reasoningFlowActive ? "reasoning-flow-active" : ""}`}>
              Thought Process
            </summary>
            <div
              className="message-reasoning-content"
              dangerouslySetInnerHTML={{ __html: renderedReasoningContent }}
            />
          </details>
        )}
        <div
          className="message-content"
          dangerouslySetInnerHTML={{ __html: renderedMainContent }}
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

      {isAssistant && (
        <div className="message-export-actions">
          <button
            type="button"
            className="message-export-btn"
            onClick={() => exportAssistantMessagePdf(renderedMainContent, message.id)}
            disabled={!mainContent.trim()}
            title="Export this reply to PDF"
          >
            导出PDF
          </button>
        </div>
      )}
    </div>
  );
}
