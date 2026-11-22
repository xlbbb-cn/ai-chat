/**
 * The desktop notification card — the `notify` window's whole UI
 * (see `src-tauri/src/notify.rs`).
 *
 * The window is created a few seconds after startup (prewarm task in
 * `lib.rs`'s setup) or on demand by `show_notify_card`, whichever comes
 * first, and shown by the latter when a chat turn settles while the user is
 * not looking at the app. This entry renders one card:
 *
 * - left: the card artwork (`notice1.png`), right: title + body
 * - click → `focus_main_from_notify` (main window back, card gone)
 * - 6 s auto-dismiss, paused while the pointer is over the card
 * - the glass look follows the main window's config: theme, palette and
 *   glass level are re-read from the config before every show, and the
 *   backdrop kind (`acrylic` / `mica` / macOS vibrancy / fallback wash)
 *   is pulled from the backend, never assumed.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getConfig } from "./api";
import type { AppConfig } from "./types";
import "./App.css";
import noticeImage from "./assets/notice1.png";

/** Payload of the backend's `notify-show` event. */
interface NotifyPayload {
  title: string;
  body: string;
  /** `"done"` | `"error"` | `"ask"` | `"confirm"` — semantic status only
   * (DESIGN.md): settled turn, failed turn, question waiting for an answer,
   * approval waiting for the user. */
  kind: string;
}

const AUTO_DISMISS_MS = 6000;
/** Must be ≥ the card's fade-out transition so the window hides after the
 * last frame, and shorter than the backend's 30 s force-hide. */
const FADE_OUT_MS = 200;

/**
 * Paint the main window's theme / palette / glass level on this document.
 * The card webview shares no state with the main one, so the tokens it uses
 * are set per show, straight from the config.
 */
function applyCardTheme(config: AppConfig) {
  const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  const theme =
    config.theme === "dark" || (config.theme !== "light" && prefersDark)
      ? "dark"
      : "light";
  const root = document.documentElement;
  root.setAttribute("data-theme", theme);
  // The same normalization as `applyTheme()` in App.tsx: an old config may
  // still name the removed Sakura palette, which has no token block here.
  root.setAttribute("data-palette", config.palette === "cream" ? "cream" : "default");
  const glass = config.glass_effect;
  root.setAttribute(
    "data-glass",
    glass === "off" || glass === "subtle" || glass === "strong" ? glass : "standard"
  );
}

/**
 * Ask the backend what backdrop ended up in effect and write the matching
 * attribute. `None` (Linux, or a failed native call) means the card paints
 * its own fallback wash — the same contract as the main window's
 * `applyWindowVibrancy`, mirrored here because this window must not wait on
 * the main document's attributes.
 */
async function applyCardBackdrop() {
  const root = document.documentElement;
  try {
    const kind = await invoke<string | null>("refresh_notify_backdrop");
    if (kind) {
      root.setAttribute("data-vibrancy", kind);
      root.removeAttribute("data-glass-fallback");
    } else {
      root.removeAttribute("data-vibrancy");
      root.setAttribute("data-glass-fallback", "");
    }
  } catch {
    root.removeAttribute("data-vibrancy");
    root.setAttribute("data-glass-fallback", "");
  }
}

function App() {
  const [item, setItem] = useState<NotifyPayload | null>(null);
  /** Controls the fade: mounted → shown on the next frame. */
  const [shown, setShown] = useState(false);
  const [hovering, setHovering] = useState(false);
  /** While the fade-out runs the content must stay on the card. */
  const itemRef = useRef<NotifyPayload | null>(null);
  /** Guards the fade-out against a show racing it mid-flight. */
  const showSeq = useRef(0);

  const hideWindow = useCallback(() => {
    invoke("hide_notify_card").catch(() => {});
  }, []);

  /** Fade out, then hide the window once the last frame is off. */
  const dismiss = useCallback(() => {
    const seq = showSeq.current;
    setShown(false);
    window.setTimeout(() => {
      // A newer show already took over the card.
      if (seq !== showSeq.current) return;
      itemRef.current = null;
      setItem(null);
      hideWindow();
    }, FADE_OUT_MS);
  }, [hideWindow]);

  useEffect(() => {
    const unlisteners: Promise<() => void>[] = [];

    // The card's tokens are per-window state: refresh them before the first
    // possible show, and then before every one.
    getConfig()
      .then((config) => {
        applyCardTheme(config);
        return applyCardBackdrop();
      })
      .catch(() => {
        document.documentElement.setAttribute("data-glass-fallback", "");
      });

    unlisteners.push(
      listen<NotifyPayload>("notify-show", (event) => {
        showSeq.current += 1;
        itemRef.current = event.payload;
        setItem(event.payload);
        setHovering(false);
        // Theme / palette / glass may have changed in the main window since
        // the last show; the backdrop kind may have changed with them.
        getConfig()
          .then((config) => {
            applyCardTheme(config);
            return applyCardBackdrop();
          })
          .catch(() => {});
        // Double rAF so the hidden frame paints before the fade starts —
        // otherwise the transition collapses into the first paint.
        requestAnimationFrame(() => requestAnimationFrame(() => setShown(true)));
      })
    );

    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()).catch(() => {}));
    };
  }, []);

  // Auto-dismiss: paused while the pointer is over the card, reset by every
  // new show (`shown` flips through false→true and `item` changes identity).
  useEffect(() => {
    if (!item || !shown || hovering) return;
    const id = window.setTimeout(dismiss, AUTO_DISMISS_MS);
    return () => window.clearTimeout(id);
  }, [item, shown, hovering, dismiss]);

  const kind =
    item?.kind === "error" || item?.kind === "ask" || item?.kind === "confirm"
      ? item.kind
      : "done";

  return (
    <div className="notify-root">
      <div
        className={`notify-card ${shown ? "notify-card-shown" : ""}`}
        role="button"
        tabIndex={-1}
        aria-label={item?.title ?? ""}
        onMouseEnter={() => setHovering(true)}
        onMouseLeave={() => setHovering(false)}
        onClick={() => {
          // One interaction: focus the main window, card gone.
          invoke("focus_main_from_notify").catch(() => {});
        }}
      >
        <img
          className="notify-card-art"
          src={noticeImage}
          alt=""
          draggable={false}
        />
        <div className="notify-card-text">
          <span className={`notify-card-title kind-${kind}`}>{item?.title}</span>
          <span className="notify-card-body">{item?.body}</span>
        </div>
      </div>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("notify-root") as HTMLElement).render(<App />);
