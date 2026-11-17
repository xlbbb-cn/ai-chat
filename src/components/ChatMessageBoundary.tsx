import { Component, type ReactNode } from "react";
import { t as text } from "../i18n";

/**
 * Keeps one broken message from taking the transcript down with it.
 *
 * The markdown pipeline already degrades to literal text when a renderer
 * throws, but an error raised *while React renders* a message (a malformed
 * figure spec, a plugin that throws outside `render`) would otherwise unmount
 * the whole tree — the window goes blank and the console shows the stack. The
 * boundary turns that into one readable row.
 *
 * It lives in its own module because `ChatMessage` is a lazy chunk: the
 * boundary has to be mounted before its child is fetched, so it cannot share
 * that chunk. Its two classes are shared primitives in `App.css` (`.markdown-
 * fallback`, `.message-render-error`) instead of `ChatMessage.css`, which
 * arrives with the transcript.
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
      <div className="message-render-error">
        <pre className="markdown-fallback">
          {text("app.messageRenderFailed", { error: this.state.error })}
        </pre>
      </div>
    );
  }
}
