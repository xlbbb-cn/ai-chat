import { useEffect, useRef, useState } from "react";
import { FontAwesomeIcon, faTriangleExclamation } from "../icons";
import { useI18n } from "../i18n";

/**
 * A transient warning or error in the chat window.
 *
 * These used to be bars inside the transcript — and before that a failure row
 * under the assistant bubble: they pushed the conversation around as they
 * appeared and stayed on screen until the next request. A notice now floats
 * over the chat window's top-left corner instead — outside the scroll
 * container, in the zero-height strip `App.tsx` renders just above the
 * transcript, so it never shifts anything.
 *
 * It leaves on a click and on its own after five seconds: opacity → 0 on the
 * shared motion curve, then the parent drops it. Only one notice is shown at a
 * time; a new one replaces the old and restarts the dwell.
 *
 * A notice may carry one action — the failed turn's "continue" is only
 * reachable from here now that the transcript holds no failure row.
 */

export type ChatNoticeTone = "error" | "warning";

export interface ChatNoticeAction {
  label: string;
  run: () => void;
}

export interface ChatNoticeState {
  text: string;
  tone: ChatNoticeTone;
  /** Bumped by the caller so a repeated identical message restarts the dwell. */
  nonce: number;
  action?: ChatNoticeAction;
}

/** How long a notice stays before it starts fading. */
const DWELL_MS = 5000;
/** Fade window — `--t-dur` (0.33s) plus a frame, after which the parent drops it. */
const FADE_MS = 340;

export function ChatNotice({
  notice,
  onDismiss,
}: {
  notice: ChatNoticeState;
  onDismiss: () => void;
}) {
  const { t } = useI18n();
  const [leaving, setLeaving] = useState(false);
  // The parent re-creates `onDismiss` freely; keeping it in a ref means the
  // fade timer never restarts on an unrelated re-render (tokens arrive at
  // several frames per second while a turn streams).
  const dismissRef = useRef(onDismiss);
  dismissRef.current = onDismiss;

  const action = notice.action ?? null;

  // A new notice restarts the dwell from zero — including the fade, so a
  // message arriving mid-fade is not swallowed by the previous one's exit.
  useEffect(() => {
    setLeaving(false);
  }, [notice.nonce]);

  // Dwell. A notice that carries an action waits for the user instead: a
  // button fading out from under the pointer that is reaching for it is worse
  // than a bar that stays until it is read.
  useEffect(() => {
    if (action || leaving) return;
    const dwell = window.setTimeout(() => setLeaving(true), DWELL_MS);
    return () => window.clearTimeout(dwell);
  }, [notice.nonce, action, leaving]);

  useEffect(() => {
    if (!leaving) return;
    const fade = window.setTimeout(() => dismissRef.current(), FADE_MS);
    return () => window.clearTimeout(fade);
  }, [leaving]);

  return (
    <div className="chat-notices" role="status" aria-live="polite">
      <div
        className={`chat-notice${leaving ? " is-leaving" : ""}`}
        data-tone={notice.tone}
      >
        {/* The bar is clamped to three lines and the whole body is the dismiss
            target, so the full text rides along as the accessible name. */}
        <button
          type="button"
          className="chat-notice-body"
          title={notice.text}
          aria-label={`${t("app.noticeDismiss")}: ${notice.text}`}
          onClick={() => setLeaving(true)}
        >
          <FontAwesomeIcon
            icon={faTriangleExclamation}
            className="chat-notice-icon"
            aria-hidden="true"
          />
          <span className="chat-notice-text">{notice.text}</span>
        </button>
        {action && (
          <button
            type="button"
            className="inline-edit-btn chat-notice-action"
            onClick={() => {
              action.run();
              setLeaving(true);
            }}
          >
            {action.label}
          </button>
        )}
      </div>
    </div>
  );
}
