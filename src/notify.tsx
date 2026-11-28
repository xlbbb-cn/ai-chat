/**
 * The companion window's whole UI (`notify.html` → `src-tauri/src/notify.rs`).
 *
 * The window wears two faces, and the backend decides which one is on screen:
 *
 * - **pet**: a small transparent square with an SVG rabbit in it, shown while
 *   the main window is not focused (Settings → Run, "Desktop pet"). Its mood
 *   follows what the app is doing — a turn in flight, a settled reply, a failed
 *   turn, a question waiting for an answer, a silent maintenance pass — and it
 *   naps on its own after a few idle minutes. Clicking it brings the main window
 *   back, dragging it moves it (the backend persists the corner).
 * - **card**: the notification card, raised when a chat turn settles while the
 *   user is not looking. Its rectangle contains the pet's bottom-right corner,
 *   so a card raised over a visible pet covers it; the pet ducks first and the
 *   frame grows into the card.
 *
 * Both faces live in one document because they are one window: geometry is the
 * backend's job, and the only thing this file owns is which layer is painted.
 *
 * The glass look follows the main window's config: theme, palette and glass
 * level are re-read from the config before every show, and the backdrop kind
 * (`acrylic` / `mica` / macOS vibrancy / fallback wash) is pulled from the
 * backend, never assumed. In the pet face the backend clears that material —
 * a backdrop paints the whole window rectangle, which would put a pane behind
 * the animal — so this file only ever *reflects* the answer it gets.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
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
   * (docs/DESIGN.md): settled turn, failed turn, question waiting for an answer,
   * approval waiting for the user. */
  kind: string;
}

/** Payload of the backend's `companion-mode` event. */
interface CompanionModePayload {
  /** `"pet"` | `"card"` | `"hidden"`. */
  mode: string;
  /** Logical edge of the pet square in px. */
  pet_size: number;
  /** `false` on macOS: the window carries an opaque system background, so the
   * pet paints its own surface instead of a hole behind it. */
  transparent: boolean;
}

/** Payload of the backend's `companion-state` event. */
interface CompanionStatePayload {
  state: string;
}

type Face = "pet" | "card";
type Mood = "idle" | "thinking" | "done" | "error" | "ask" | "dreaming";

const AUTO_DISMISS_MS = 6000;
/** Must be ≥ the card's fade-out transition so the window hides after the
 * last frame, and shorter than the backend's 30 s force-hide. */
const FADE_OUT_MS = 200;
/** How long a settled mood stays on the pet's face before it goes back to
 * `idle`. A mood the backend keeps pushing (a running turn) simply outlives
 * the timer, because every event restarts it. */
const MOOD_TTL_MS: Record<"done" | "error" | "ask", number> = {
  done: 2400,
  error: 4200,
  ask: 8000,
};
/** Idle time before the pet dozes off (it wakes on the next mood). */
const SLEEP_AFTER_MS = 4 * 60 * 1000;
/** Movement in px that turns a press on the pet into a window drag. */
const DRAG_THRESHOLD = 5;

/**
 * Paint the main window's theme / palette / glass level on this document.
 * The companion webview shares no state with the main one, so the tokens it
 * uses are set per show, straight from the config.
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
  root.setAttribute(
    "data-palette",
    config.palette === "cream" || config.palette === "macaron" ? config.palette : "default"
  );
  const glass = config.glass_effect;
  root.setAttribute(
    "data-glass",
    glass === "off" || glass === "subtle" || glass === "strong" ? glass : "standard"
  );
}

/**
 * Ask the backend what backdrop ended up in effect and write the matching
 * attribute. `None` (the pet face, Linux, or a failed native call) means no
 * material is in effect — the pet paints nothing behind itself, and the card
 * keeps its fallback wash.
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

function readConfigIntoTokens() {
  getConfig()
    .then((config) => {
      applyCardTheme(config);
      return applyCardBackdrop();
    })
    .catch(() => {});
}

/**
 * The rabbit, as a single-colour silhouette with the eyes cut out of it.
 *
 * The empty state's face (`App.tsx`, `DREAM_FACE`) fills its eyes with
 * `--c-bg` — the page behind it. A desktop pet has no page behind it: the eyes
 * are holes cut through the silhouette (an SVG mask), so the wallpaper shows
 * through them exactly like it shows around the body. The mask's own fills are
 * alpha math, not design tokens.
 */
const PET_SVG = (
  <svg className="pet-svg" viewBox="0 0 64 64" role="presentation" focusable="false">
    <defs>
      <mask
        id="pet-eye-cut"
        maskUnits="userSpaceOnUse"
        x="0"
        y="0"
        width="64"
        height="64"
      >
        <rect x="0" y="0" width="64" height="64" fill="#fff" />
        <ellipse className="pet-eye" cx="26" cy="31" rx="2.4" ry="5" fill="#000" />
        <ellipse className="pet-eye" cx="38" cy="31" rx="2.4" ry="5" fill="#000" />
      </mask>
    </defs>
    <g className="pet-body" mask="url(#pet-eye-cut)">
      <ellipse
        className="pet-fill pet-ear pet-ear-l"
        cx="24.2"
        cy="15.5"
        rx="4.4"
        ry="11.6"
        transform="rotate(-8 24.2 27)"
      />
      <ellipse
        className="pet-fill pet-ear pet-ear-r"
        cx="39.8"
        cy="15.5"
        rx="4.4"
        ry="11.6"
        transform="rotate(8 39.8 27)"
      />
      <ellipse className="pet-fill" cx="25.5" cy="52" rx="5.2" ry="3.1" />
      <ellipse className="pet-fill" cx="38.5" cy="52" rx="5.2" ry="3.1" />
      <ellipse className="pet-fill" cx="32" cy="34.5" rx="16.5" ry="14.5" />
    </g>
  </svg>
);

function App() {
  // ── Card face ────────────────────────────────────────────────────────────
  const [item, setItem] = useState<NotifyPayload | null>(null);
  /** Controls the fade: mounted → shown on the next frame. */
  const [shown, setShown] = useState(false);
  const [hovering, setHovering] = useState(false);
  /** While the fade-out runs the content must stay on the card. */
  const itemRef = useRef<NotifyPayload | null>(null);
  /** Guards the fade-out against a show racing it mid-flight. */
  const showSeq = useRef(0);

  // ── Pet face ─────────────────────────────────────────────────────────────
  const [face, setFace] = useState<Face>("pet");
  const [mood, setMood] = useState<Mood>("idle");
  const [sleeping, setSleeping] = useState(false);
  /** The pet is ducking because a card is about to take the window. */
  const [leaving, setLeaving] = useState(false);
  /** Bumped after a card hands the window back, to replay the pop-in. */
  const [popSeq, setPopSeq] = useState(0);
  /** `false` on macOS, where the window has no transparent background. */
  const [transparent, setTransparent] = useState(true);
  /** The pointer moved far enough that the press became a window drag. */
  const suppressClick = useRef(false);
  const pressOrigin = useRef<{ x: number; y: number } | null>(null);
  const dragging = useRef(false);

  // The face is a document-level attribute: the body fill and the glass wash
  // differ between the two, and both are painted by `body`, not by a component.
  useEffect(() => {
    const root = document.documentElement;
    root.setAttribute("data-companion", face);
    return () => root.removeAttribute("data-companion");
  }, [face]);

  useEffect(() => {
    document.documentElement.setAttribute(
      "data-companion-surface",
      transparent ? "transparent" : "opaque"
    );
  }, [transparent]);

  // Pet size comes from the backend, which owns the window geometry.
  useEffect(() => {
    const root = document.documentElement;
    const apply = (size: number) =>
      root.style.setProperty("--pet-size", `${size}px`);
    const unlisten = listen<CompanionModePayload>("companion-mode", (event) => {
      if (event.payload.pet_size) apply(event.payload.pet_size);
      setTransparent(event.payload.transparent !== false);
    });
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  /** Fade out, then hand the window back to the backend (pet, or hidden). */
  const dismiss = useCallback(() => {
    const seq = showSeq.current;
    setShown(false);
    window.setTimeout(() => {
      // A newer show already took over the card.
      if (seq !== showSeq.current) return;
      itemRef.current = null;
      setItem(null);
      invoke("hide_notify_card").catch(() => {});
    }, FADE_OUT_MS);
  }, []);

  useEffect(() => {
    const unlisteners: Promise<() => void>[] = [];

    // The tokens are per-window state: refresh them before the first possible
    // show, and then before every one.
    readConfigIntoTokens();

    unlisteners.push(
      listen<NotifyPayload>("notify-show", (event) => {
        showSeq.current += 1;
        itemRef.current = event.payload;
        setItem(event.payload);
        setHovering(false);
        setLeaving(false);
        setFace("card");
        // Theme / palette / glass may have changed in the main window since
        // the last show; the backdrop kind may have changed with them.
        readConfigIntoTokens();
        // Double rAF so the hidden frame paints before the fade starts —
        // otherwise the transition collapses into the first paint.
        requestAnimationFrame(() => requestAnimationFrame(() => setShown(true)));
      })
    );

    unlisteners.push(
      listen<CompanionModePayload>("companion-mode", (event) => {
        const next = event.payload.mode;
        if (next === "card") {
          // The frame is still the pet's: duck now, the resize follows in
          // `PET_DUCK_MS` (the backend waits for exactly this animation).
          setLeaving(true);
          return;
        }
        if (next === "pet") {
          setFace("pet");
          setLeaving(false);
          setPopSeq((seq) => seq + 1);
          setSleeping(false);
          readConfigIntoTokens();
        }
      })
    );

    unlisteners.push(
      listen<CompanionStatePayload>("companion-state", (event) => {
        const next = event.payload.state as Mood;
        if (!(next in MOOD_TTL_MS) && next !== "idle" && next !== "thinking" && next !== "dreaming") {
          return;
        }
        setMood((current) => {
          // `idle` is a background level, not news: it must not wipe a settled
          // reply / failure / question the user has not looked at yet — those
          // decay on their own timer.
          if (
            next === "idle" &&
            (current === "done" || current === "error" || current === "ask")
          ) {
            return current;
          }
          return next;
        });
        setSleeping(false);
      })
    );

    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()).catch(() => {}));
    };
  }, []);

  // Auto-dismiss: paused while the pointer is over the card, reset by every
  // new show (`shown` flips through false→true and `item` changes identity).
  useEffect(() => {
    if (!item || !shown || hovering || face !== "card") return;
    const id = window.setTimeout(dismiss, AUTO_DISMISS_MS);
    return () => window.clearTimeout(id);
  }, [item, shown, hovering, face, dismiss]);

  // A settled mood decays back to idle; a mood the backend keeps pushing is
  // refreshed by its next event, which re-arms this timer.
  useEffect(() => {
    if (mood !== "done" && mood !== "error" && mood !== "ask") return;
    const id = window.setTimeout(() => setMood("idle"), MOOD_TTL_MS[mood]);
    return () => window.clearTimeout(id);
  }, [mood]);

  // Doze off after a long idle stretch — the pet's own clock, no backend event.
  useEffect(() => {
    if (mood !== "idle" || sleeping) return;
    const id = window.setTimeout(() => setSleeping(true), SLEEP_AFTER_MS);
    return () => window.clearTimeout(id);
  }, [mood, sleeping]);

  const onPetPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    pressOrigin.current = { x: event.clientX, y: event.clientY };
    suppressClick.current = false;
    dragging.current = false;
  };

  const onPetPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const origin = pressOrigin.current;
    if (!origin || dragging.current) return;
    if (
      Math.abs(event.clientX - origin.x) + Math.abs(event.clientY - origin.y) <
      DRAG_THRESHOLD
    ) {
      return;
    }
    // The pointer left the square: this is a drag, not a click. `startDragging`
    // hands the move to the window manager, which is the only thing that can
    // move a frameless always-on-top window.
    dragging.current = true;
    suppressClick.current = true;
    invoke("pet_begin_drag").catch(() => {});
  };

  const endPetPress = () => {
    pressOrigin.current = null;
    dragging.current = false;
  };

  const kind =
    item?.kind === "error" || item?.kind === "ask" || item?.kind === "confirm"
      ? item.kind
      : "done";

  return (
    <div
      className="companion-root"
      data-mood={mood}
      data-sleeping={sleeping ? "true" : "false"}
    >
      <div
        className={`pet-layer${leaving ? " pet-layer-leaving" : ""}`}
        key={`pet-${popSeq}`}
        role="button"
        tabIndex={-1}
        aria-label="Rabbit Chat Studio"
        onPointerDown={onPetPointerDown}
        onPointerMove={onPetPointerMove}
        onPointerUp={endPetPress}
        onPointerCancel={endPetPress}
        onClick={() => {
          if (suppressClick.current) {
            suppressClick.current = false;
            return;
          }
          invoke("focus_main_from_notify").catch(() => {});
        }}
      >
        {PET_SVG}
        <span className="pet-dots" aria-hidden="true">
          <i />
          <i />
          <i />
        </span>
        <span className="pet-zzz" aria-hidden="true">
          <i>z</i>
          <i>z</i>
          <i>z</i>
        </span>
      </div>

      {item && (
        <div
          className={`notify-card ${shown ? "notify-card-shown" : ""}`}
          role="button"
          tabIndex={-1}
          aria-label={item.title}
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
            <span className={`notify-card-title kind-${kind}`}>{item.title}</span>
            <span className="notify-card-body">{item.body}</span>
          </div>
        </div>
      )}
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("notify-root") as HTMLElement).render(<App />);
