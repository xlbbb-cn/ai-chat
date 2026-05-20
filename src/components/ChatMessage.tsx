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
}

export function ChatMessage({ message }: Props) {
  const isUser = message.role === "user";
  return (
    <div className={`chat-message ${isUser ? "user" : "assistant"}`}>
      <div className="message-role">{isUser ? "You" : "Assistant"}</div>
      {message.reasoning_content && (
        <details className="message-reasoning">
          <summary>Thought Process</summary>
          <div
            className="message-reasoning-content"
            dangerouslySetInnerHTML={{ __html: md.render(message.reasoning_content) }}
          />
        </details>
      )}
      <div
        className="message-content"
        dangerouslySetInnerHTML={{ __html: md.render(message.content) }}
        onClick={(e) => {
          const target = e.target as HTMLElement;
          const link = target.closest("a");
          if (link) {
            e.preventDefault();
          }
        }}
      />
      {message.streaming && <span className="cursor">|</span>}
    </div>
  );
}
