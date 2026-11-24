/**
 * The plugin manager window — Settings → Run → Plugin manager → "Open plugin manager".
 *
 * A separate webview (`plugin-manager.html` → this entry, see
 * `src-tauri/src/plugins/manager.rs`), created on demand by the
 * `open_plugin_manager` command. Like the notification card it shares no React
 * state with the main window, so the tokens it paints with are read from the
 * config before the first frame; unlike it, the panel is a full document with
 * the app's glass canvas.
 *
 * The language is taken from the persisted choice (the main window writes it to
 * `localStorage`), falling back to `config.language`, so both windows always
 * speak the same language.
 */
import { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { getConfig } from "./api";
import type { AppConfig } from "./types";
import { I18nProvider, isLocale } from "./i18n";
import { PluginManagerPanel } from "./components/PluginManagerPanel";
import "./App.css";

/**
 * Paint a startup failure into the page.
 *
 * A separate webview has no console anyone is watching, so anything that throws
 * before React mounts would show up as an eternally blank window.
 */
function showFatal(message: string) {
  const root = document.getElementById("plugin-manager-root");
  if (!root) return;
  root.innerHTML = "";
  const box = document.createElement("pre");
  box.className = "pm-fatal";
  box.textContent = `Plugin manager failed to start\n\n${message}`;
  root.appendChild(box);
  report(`fatal ${message.slice(0, 400)}`);
}

window.addEventListener("error", (event) => {
  const stack = event.error instanceof Error ? event.error.stack : undefined;
  showFatal(stack ?? `${event.message} (${event.filename}:${event.lineno})`);
});

/**
 * Breadcrumb for `%TEMP%/rabbit-plugin-window.log` (Rust side: `trace`).
 *
 * A webview has no console anyone can read back, so the page reports what it
 * got as far as; when the window stays blank the log says whether the module
 * even ran.
 */
function report(message: string) {
  void invoke("plugin_window_report", { message }).catch(() => {});
}

report("module loaded");

/** Same normalization as `applyTheme()` in App.tsx / `notify.tsx`. */
function applyWindowTheme(config: AppConfig) {
  const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  const theme =
    config.theme === "dark" || (config.theme !== "light" && prefersDark)
      ? "dark"
      : "light";
  const root = document.documentElement;
  root.setAttribute("data-theme", theme);
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

function Root() {
  const [ready, setReady] = useState(false);

  useEffect(() => {
    // A stalled IPC call must not keep the window blank forever.
    const deadline = new Promise<void>((resolve) => window.setTimeout(resolve, 1500));
    const load = getConfig()
      .then((config) => {
        applyWindowTheme(config);
        report("theme applied");
        // The panel renders through `I18nProvider`, which reads the cached
        // locale first. Seed it from the saved config so a fresh profile still
        // opens in the configured language.
        try {
          if (isLocale(config.language)) {
            window.localStorage.setItem("ai-chat.locale", config.language);
          }
        } catch {
          /* storage may be unavailable — the provider's fallback applies */
        }
      })
      .catch(() => {})
      .finally(() => setReady(true));
    void Promise.race([load, deadline]);
  }, []);

  // Hold the first paint until the theme is known; a flash of the wrong
  // palette in a separate window is much more visible than in a card.
  if (!ready) return <div className="pm-root pm-boot" aria-busy="true" />;

  return (
    <I18nProvider>
      <PluginManagerPanel />
    </I18nProvider>
  );
}

ReactDOM.createRoot(
  document.getElementById("plugin-manager-root") as HTMLElement
).render(<Root />);
