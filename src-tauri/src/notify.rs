//! The desktop notification card — a second, frameless webview window.
//!
//! When a chat turn settles (done or failed) while the main window is
//! minimized or not focused, the frontend calls [`show_notify_card`]: this
//! module creates-or-reuses the `notify` window — prewarmed a few seconds
//! after startup (`lib.rs`'s setup), created on demand before that — places it
//! at the bottom-right corner of the screen, pushes the title / body to its
//! webview and shows it. The card
//! is the app's own UI (see `notify.html` / `src/notify.tsx` and the
//! `.notify-*` rules in `src/App.css`), so its frosted-glass card follows the
//! same tokens and glass levels as the main frame — an OS toast would be
//! drawn by Windows and could not carry either the app's glass look or the
//! card's artwork.
//!
//! Windows paints the backdrop with the same Acrylic recipe as the main
//! window (`crate::apply_window_backdrop`); macOS gets a real vibrancy view
//! (a toast card is exactly the transient surface the material is for); Linux
//! has no compositor API to ask, so the card falls back to the painted wash
//! (`data-glass-fallback`), like the main frame does.
//!
//! The same window is also the **desktop pet** (Settings → Run, "Desktop pet"):
//! a small always-on-top square with an SVG rabbit in it that appears while the
//! main window is not focused and reacts to what the app is doing. One window,
//! two faces — [`Mode`] is the switch:
//!
//! - `Pet`: pet-sized, fully transparent (no glass material, no page fill,
//!   `data-companion="pet"` in the document), placed at the user's own anchor.
//! - `Card`: the notification card as before, placed so that its rectangle
//!   contains the pet's bottom-right corner — a card raised while the pet is on
//!   screen literally covers it (the pet ducks first, then the frame grows).
//!
//! Behaviour contract:
//! - the window never steals focus on show (`focus: false` + a hidden show)
//! - the card is hidden again by its own 6 s timer (hover pauses it), by a
//!   click (which also focuses the main window), or when the main window gains
//!   focus (`on_main_focus` from `lib.rs`'s window event handler). Dismissing a
//!   card while the pet is enabled hands the window back to the pet instead of
//!   hiding it.
//! - the pet follows the config (`desktop_pet`, `desktop_pet_hide_when_focused`,
//!   `desktop_pet_scale`, `desktop_pet_anchor`) through [`sync_pet`], which the
//!   focus handler, the config writer and the prewarm task all call.
//! - its `CloseRequested` is intercepted, so a stray close only hides it and the
//!   card survives app-lifetime; the single destroy path is the main window's
//!   teardown in `lib.rs`, which removes the card first so the runtime's
//!   `windows.is_empty()` exit can fire on its own
//!
//! The pet's click / drag / mood surface is four small commands in
//! `ipc/notify.rs`; nothing here needs a new window permission, because every
//! window operation stays on the Rust side (see `capabilities/notify.json`).

use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

pub const NOTIFY_WINDOW: &str = "notify";

/// The card webview is alive and its listeners are mounted. Flipped by the
/// mount call of `refresh_notify_backdrop` (notify.tsx always asks for the
/// backdrop state on mount); an on-demand card waits for it before the first
/// show, so a freshly created webview never shows blank and never loses the
/// `notify-show` event.
static NOTIFY_READY: AtomicBool = AtomicBool::new(false);

/// Serializes "check-then-build" across the two creators (the prewarm task and
/// an early toast). Without it both can observe the window missing and race
/// two same-label builds — one losing. The lock is never held across the
/// mount wait, so it cannot deadlock.
static NOTIFY_CREATE_LOCK: Mutex<()> = Mutex::new(());

/// Logical size of the card. Must match the `.notify-card` design in
/// `src/App.css` (fixed on purpose: a toast card does not resize).
const CARD_WIDTH: f64 = 348.0;
const CARD_HEIGHT: f64 = 112.0;
/// Gap between the card and the work-area edges (physical px).
const MARGIN_RIGHT: i32 = 12;
const MARGIN_BOTTOM: i32 = 12;
/// Extra bottom margin on macOS / Linux, where no work-area query exists —
/// covers a bottom dock / panel without pretending to measure one.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const SHELL_MARGIN_BOTTOM: i32 = 48;
/// Generation counter for the hard-cap guard: only the spawn belonging to the
/// latest show may force-hide the window.
static NOTIFY_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize, Clone)]
struct NotifyPayload {
    title: String,
    body: String,
    kind: String,
}

// ─── Companion: the pet face of the same window ──────────────────────────────

/// Pet edge in logical px per `desktop_pet_scale` value.
const PET_SMALL: f64 = 96.0;
const PET_MEDIUM: f64 = 120.0;
const PET_LARGE: f64 = 160.0;
/// How long the pet takes to duck out of the way before the frame grows into
/// the card. Must match `.pet-layer-leaving` in `src/App.css`.
const PET_DUCK_MS: u64 = 180;
/// Quiet period after the last `Moved` event before a dragged anchor is
/// written to the config — a drag is a burst of window messages.
const ANCHOR_SAVE_MS: u64 = 600;

/// What the one companion window is doing right now.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Off screen: never shown yet, or the pet is disabled and no card is up.
    Hidden,
    /// The desktop pet: small, fully transparent, always-on-top.
    Pet,
    /// The notification card.
    Card,
}

impl Mode {
    fn as_u8(self) -> u8 {
        match self {
            Mode::Hidden => 0,
            Mode::Pet => 1,
            Mode::Card => 2,
        }
    }

    fn from_u8(raw: u8) -> Self {
        match raw {
            1 => Mode::Pet,
            2 => Mode::Card,
            _ => Mode::Hidden,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Mode::Hidden => "hidden",
            Mode::Pet => "pet",
            Mode::Card => "card",
        }
    }
}

/// The window's current face. One atomic rather than a mutex: every reader only
/// compares it, and the transitions run on the window-event thread.
static MODE: AtomicU8 = AtomicU8::new(0);
/// Generation of the debounced anchor write: only the newest `Moved` burst may
/// persist, so a fast drag writes the config once.
static ANCHOR_SEQ: AtomicU64 = AtomicU64::new(0);

fn current_mode() -> Mode {
    Mode::from_u8(MODE.load(Ordering::SeqCst))
}

/// The pet face's settings, with the anchor already resolved to a concrete
/// bottom-right corner (the config's value, or the default corner when the user
/// never dragged the pet).
#[derive(Clone, Copy)]
struct PetSettings {
    enabled: bool,
    hide_when_focused: bool,
    /// Logical edge in px.
    size: f64,
    /// Physical bottom-right corner of the pet window.
    anchor: (i32, i32),
}

/// Create the notification card window, or return the existing one.
///
/// The window used to be declared in `tauri.conf.json`, which paid a full
/// second webview (renderer process, the 355 KB card artwork, the shared
/// stylesheet) before the main window's first paint. It is now created by the
/// prewarm task a few seconds after startup (`lib.rs`'s setup), and on demand
/// here when a toast arrives earlier than that.
///
/// Blocking (WebView2 window creation) — call from `spawn_blocking` or a sync
/// command thread, never directly inside an async task body.
pub fn ensure_notify_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    // Poisoning a mutex that only guards creation is harmless — proceed.
    let _guard = NOTIFY_CREATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(window) = app.get_webview_window(NOTIFY_WINDOW) {
        return Ok(window);
    }
    let builder = tauri::WebviewWindowBuilder::new(
        app,
        NOTIFY_WINDOW,
        tauri::WebviewUrl::App("notify.html".into()),
    )
    // Mirrors the previous `notify` entry in `tauri.conf.json` exactly:
    // hidden, frameless, no shadow, outside the task bar, never activated.
    // The show path (`window.show()`) relies on this — the user's current
    // application must keep the foreground.
    .title("Rabbit Chat Studio")
    .inner_size(CARD_WIDTH, CARD_HEIGHT)
    .visible(false)
    .decorations(false)
    .shadow(false);
    // `WebviewWindowBuilder::transparent` only exists off macOS unless the
    // `macos-private-api` feature is on (it compiles wry's private
    // `drawsBackground` KVC). This app does not enable that feature — which is
    // also how the previous `tauri.conf.json` entry behaved: the config path
    // skipped the transparent call on macOS and only warned in a debug build.
    // So the card is transparent everywhere else, and on macOS it keeps the
    // vibrancy material / painted wash as before.
    #[cfg(not(target_os = "macos"))]
    let builder = builder.transparent(true);

    builder
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true)
        .focused(false)
        .always_on_top(true)
        .build()
        .map_err(|e| e.to_string())
}

/// Show the notification card with `title` / `body`.
///
/// `kind` is `"done"` or `"error"` — the card UI colours the title from it
/// (semantic status only, per docs/DESIGN.md).
pub(crate) async fn show_notify_card(
    app: AppHandle,
    title: String,
    body: String,
    kind: String,
) -> Result<(), String> {
    // The card may not exist yet: the prewarm task creates it a few seconds
    // after startup, and an earlier toast creates it here. A freshly created
    // webview needs its page load before it can render or listen, so the
    // first use waits for the mount signal (`NOTIFY_READY`, set by the card's
    // mount call of `refresh_notify_backdrop`) instead of showing a blank
    // card and losing the event. A card that never answers still shows after
    // the deadline and is force-hidden 30 s later, as before.
    let created = app.get_webview_window(NOTIFY_WINDOW).is_none();
    let app_for_build = app.clone();
    let window = tauri::async_runtime::spawn_blocking(move || ensure_notify_window(&app_for_build))
        .await
        .map_err(|e| format!("notify window task failed: {e}"))??;
    if created {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !NOTIFY_READY.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    // The window may currently be the pet. Hand it to the card face first (the
    // document plays the pet's duck), then let the frame grow: a card that
    // appeared over a live animal in one frame would read as a glitch.
    let was_pet = current_mode() == Mode::Pet && window.is_visible().unwrap_or(false);
    if was_pet {
        set_mode(&app, Mode::Card);
        tokio::time::sleep(std::time::Duration::from_millis(PET_DUCK_MS)).await;
    }
    let settings = pet_settings(&app);
    if let Err(err) = place_card(&app, &window, &settings) {
        // A stale geometry is better than no notification; the card still
        // shows wherever the window happens to be.
        eprintln!("notify: placement failed: {err}");
    }
    // Card face: the frosted material follows the main window again (the pet
    // face keeps it cleared).
    apply_notify_backdrop(&app);
    let _ = window.set_always_on_top(true);
    app.emit_to(
        NOTIFY_WINDOW,
        crate::events::NOTIFY_SHOW,
        NotifyPayload { title, body, kind },
    )
    .map_err(|e| e.to_string())?;
    // The card webview listens for `notify-show` to render; the window itself
    // is only the canvas. `show()` never activates it (tao uses
    // SW_SHOWNOACTIVATE, and the builder carries `focused: false`), so the
    // user's current application keeps the foreground.
    window.show().map_err(|e| e.to_string())?;
    if !was_pet {
        set_mode(&app, Mode::Card);
    }

    // Insurance against a card webview that never answered (load failure, dev
    // reload): the card is always gone after 30 s, hovered or not. A newer
    // show bumps the generation, so only the latest card can be force-hidden.
    let seq = NOTIFY_SEQ.fetch_add(1, Ordering::SeqCst);
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        if NOTIFY_SEQ.load(Ordering::SeqCst) != seq {
            return;
        }
        // Dismiss the way the card's own timer does, so the pet gets the
        // window back instead of the frame simply vanishing.
        if current_mode() == Mode::Card {
            dismiss_card(&handle);
        }
    });
    Ok(())
}

/// Dismiss the card. Called by the card UI itself (auto-dismiss timer) and by
/// the 30 s backstop; hands the window back to the pet when it is enabled.
pub(crate) fn hide_notify_card(app: AppHandle) {
    dismiss_card(&app);
}

/// Clicking the card or the pet brings the main window back, un-minimized and
/// focused — the companion's one interaction, mirroring a system toast click.
/// The card is dismissed here rather than waiting for the focus event, so the
/// window is never both up and unfocused.
pub(crate) fn focus_main_from_notify(app: AppHandle) {
    if current_mode() == Mode::Card {
        dismiss_card(&app);
    }
    crate::focus_main_window(&app);
}

/// The card window asks for the current backdrop state on mount and before
/// every show, instead of being pushed one: an `eval` from `setup` can race
/// the webview load and the attribute would be silently lost. Returns the
/// backdrop kind in effect (`mica` / `acrylic` / `vibrancy`) or `None` —
/// `None` means the card paints its own fallback wash.
pub(crate) fn refresh_notify_backdrop(app: AppHandle) -> Option<String> {
    // The mount call doubles as the readiness signal for an on-demand card
    // (see `show_notify_card`): from here the webview can receive events.
    NOTIFY_READY.store(true, Ordering::SeqCst);
    apply_notify_backdrop(&app)
}

/// Round the card window the way Windows 11 rounds every app window — the
/// main frame included: `DWMWCP_ROUND` clips the window, and the Acrylic
/// backdrop it carries, at the system's own corner radius (the `--r-toast`
/// value in `src/App.css`, which the painted frame also uses). Without it the
/// frameless card (`shadow: false`, no WS_THICKFRAME) stays square and the
/// backdrop sheet pokes out past the painted frame's rounded corners. Older
/// Windows refuses the attribute and changes nothing; the painted `--r-toast`
/// corner remains.
#[cfg(target_os = "windows")]
fn round_window(window: &WebviewWindow) {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        DWM_WINDOW_CORNER_PREFERENCE,
    };

    // Tauri's `HWND` may be a different windows-sys revision's type; bridge
    // through `usize` the same way `work_area_bottom_right` does.
    let Some(hwnd) = window
        .hwnd()
        .ok()
        .map(|h| h.0 as usize as *mut core::ffi::c_void)
    else {
        return;
    };
    let hwnd: HWND = hwnd;
    let preference: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &preference as *const DWM_WINDOW_CORNER_PREFERENCE as *const core::ffi::c_void,
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
    }
}

/// Apply the frosted-glass backdrop to the notification window (idempotent)
/// and write the matching attribute into its document.
///
/// Called at card mount, before every show and whenever the main window's
/// `set_window_vibrancy` command runs, so a Settings change re-tints the card
/// on the spot.
pub fn apply_notify_backdrop(app: &AppHandle) -> Option<String> {
    let window = app.get_webview_window(NOTIFY_WINDOW)?;
    // The pet face must stay transparent: a backdrop material paints the whole
    // window rectangle, which would put a blurred pane behind the animal. This
    // also covers `set_window_vibrancy`, which re-tints the card from Settings
    // while the pet may be the face on screen.
    if current_mode() == Mode::Pet {
        return clear_notify_backdrop(app);
    }
    // Same silhouette as the main frame: the OS rounds the card window (and
    // the backdrop sheet it carries) at the app's own window corner radius.
    // Idempotent, so it is safe to repeat on every backdrop refresh.
    #[cfg(target_os = "windows")]
    round_window(&window);
    #[allow(unused_mut)]
    let (mut level, dark) = {
        let state = app.try_state::<crate::AppState>()?;
        let config = state.config.lock().unwrap();
        (
            config
                .glass_effect
                .clone()
                .unwrap_or_else(|| "standard".to_string()),
            config.theme.as_deref() == Some("dark"),
        )
    };
    // Windows carries the main window's recipe over; the non-Windows version
    // of `crate::apply_window_backdrop` always reports `None`, so macOS gets
    // its real vibrancy below and Linux keeps `None` (fallback wash).
    #[allow(unused_mut)]
    let mut kind = crate::apply_window_backdrop(&window, &level, dark);
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{
            apply_vibrancy, clear_vibrancy, NSVisualEffectMaterial, NSVisualEffectState,
        };
        if level == "off" {
            // Same intent as the Windows branch of `apply_window_backdrop`:
            // "off" leaves no previously applied backdrop behind.
            let _ = clear_vibrancy(&window);
            kind = None;
        } else if apply_vibrancy(
            &window,
            NSVisualEffectMaterial::Popover,
            Some(NSVisualEffectState::FollowsWindowActiveState),
            // Matches `--r-toast` — the app's window corner radius, which the
            // painted card uses too, so the material's corner is the painted
            // corner.
            Some(8.0),
        )
        .is_ok()
        {
            kind = Some("vibrancy");
        }
    }
    let script = match kind {
        Some(k) => format!(
            "document.documentElement.setAttribute('data-vibrancy','{k}');\
             document.documentElement.removeAttribute('data-glass-fallback');"
        ),
        None => "document.documentElement.removeAttribute('data-vibrancy');\
                 document.documentElement.setAttribute('data-glass-fallback','');"
            .to_string(),
    };
    let _ = window.eval(&script);
    kind.map(str::to_string)
}

// ─── Companion faces: geometry, state and the two show paths ─────────────────

/// Resolve the pet settings from the config (defaults when the config is
/// unavailable, e.g. during teardown).
fn pet_settings(app: &AppHandle) -> PetSettings {
    let mut settings = PetSettings {
        enabled: false,
        hide_when_focused: true,
        size: PET_MEDIUM,
        anchor: default_pet_anchor(app),
    };
    let Some(state) = app.try_state::<crate::AppState>() else {
        return settings;
    };
    let config = state.config.lock().unwrap();
    // One pet per desktop: a second instance would raise a second animal in the
    // same corner, so the pet only exists under single-instance mode. Settings →
    // Run shows the same rule as a locked checkbox.
    settings.enabled = config.desktop_pet && config.single_instance;
    settings.hide_when_focused = config.desktop_pet_hide_when_focused;
    settings.size = match config.desktop_pet_scale.as_deref() {
        Some("s") => PET_SMALL,
        Some("l") => PET_LARGE,
        _ => PET_MEDIUM,
    };
    if let Some(anchor) = config.desktop_pet_anchor {
        // A saved corner only survives while a monitor still covers it: after
        // unplugging a display the pet has to come back on screen, not off it.
        if anchor_on_screen(app, anchor[0], anchor[1]) {
            settings.anchor = (anchor[0], anchor[1]);
        }
    }
    settings
}

/// Bottom-right corner of the pet's default slot — the card's own corner, so
/// both faces start in the same place and a card raised over a freshly shown
/// pet covers it exactly.
fn default_pet_anchor(app: &AppHandle) -> (i32, i32) {
    match work_area_bottom_right(app) {
        Ok((right, bottom, extra_bottom)) => {
            (right - MARGIN_RIGHT, bottom - extra_bottom - MARGIN_BOTTOM)
        }
        Err(_) => (1280, 800),
    }
}

/// Is `(x, y)` inside one of the monitors the OS reports?
fn anchor_on_screen(app: &AppHandle, x: i32, y: i32) -> bool {
    let Ok(monitors) = app.available_monitors() else {
        return false;
    };
    if monitors.is_empty() {
        return false;
    }
    monitors.iter().any(|monitor| {
        let position = monitor.position();
        let size = monitor.size();
        x >= position.x
            && x <= position.x + size.width as i32
            && y >= position.y
            && y <= position.y + size.height as i32
    })
}

/// Physical top-left corner of the pet window.
fn pet_origin(window: &WebviewWindow, settings: &PetSettings) -> (i32, i32) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let edge = (settings.size * scale).round() as i32;
    (settings.anchor.0 - edge, settings.anchor.1 - edge)
}

/// Physical top-left corner of the card: its bottom-right corner sits on the
/// pet's, so a card raised while the pet is up covers it instead of landing
/// somewhere else on the screen. Clamped into the work area.
fn card_origin(app: &AppHandle, window: &WebviewWindow, settings: &PetSettings) -> (i32, i32) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let width = (CARD_WIDTH * scale).round() as i32;
    let height = (CARD_HEIGHT * scale).round() as i32;
    let (right, bottom, extra_bottom) =
        work_area_bottom_right(app).unwrap_or((settings.anchor.0, settings.anchor.1, 0));
    let x = (settings.anchor.0 - width).clamp(0, (right - width).max(0));
    let y = (settings.anchor.1 - height).clamp(0, (bottom - extra_bottom - height).max(0));
    (x, y)
}

/// Size and place the window for the card face.
fn place_card(
    app: &AppHandle,
    window: &WebviewWindow,
    settings: &PetSettings,
) -> Result<(), String> {
    window
        .set_size(tauri::LogicalSize::new(CARD_WIDTH, CARD_HEIGHT))
        .map_err(|e| e.to_string())?;
    let (x, y) = card_origin(app, window, settings);
    window
        .set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())
}

/// Store the face and tell the document, so the pet can play its exit before
/// the frame changes size.
fn set_mode(app: &AppHandle, mode: Mode) {
    MODE.store(mode.as_u8(), Ordering::SeqCst);
    let payload = crate::events::CompanionModePayload {
        mode: mode.as_str().to_string(),
        pet_size: pet_settings(app).size.round() as u32,
        transparent: cfg!(not(target_os = "macos")),
    };
    let _ = app.emit_to(NOTIFY_WINDOW, crate::events::COMPANION_MODE, payload);
}

/// Run `action` with the companion window, creating it first when the pet was
/// switched on before the prewarm task ran.
///
/// The missing-window branch hops off the caller's thread: `on_window_event`
/// runs on the UI thread, where building a webview would deadlock against the
/// message loop it is handling.
fn with_companion_window<F>(app: &AppHandle, action: F)
where
    F: FnOnce(&AppHandle, &WebviewWindow) + Send + 'static,
{
    if let Some(window) = app.get_webview_window(NOTIFY_WINDOW) {
        action(app, &window);
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let builder = app.clone();
        let built =
            tauri::async_runtime::spawn_blocking(move || ensure_notify_window(&builder)).await;
        if let Ok(Ok(window)) = built {
            action(&app, &window);
        }
    });
}

fn main_window_focused(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false)
}

/// Is the pet face allowed on screen right now?
fn pet_allowed(app: &AppHandle, settings: &PetSettings) -> bool {
    settings.enabled && (!settings.hide_when_focused || !main_window_focused(app))
}

/// Put the window in the pet face: small, at the user's anchor, transparent.
fn show_pet_window(app: &AppHandle, settings: &PetSettings) {
    let settings = *settings;
    with_companion_window(app, move |app, window| {
        let already = current_mode() == Mode::Pet && window.is_visible().unwrap_or(false);
        let _ = window.set_size(tauri::LogicalSize::new(settings.size, settings.size));
        let (x, y) = pet_origin(window, &settings);
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
        if already {
            // Same face, same place: re-emitting the mode would replay the
            // document's pop-in on every focus event. The geometry above is all
            // a repeated call (or a size change) needs.
            return;
        }
        clear_notify_backdrop(app);
        set_mode(app, Mode::Pet);
        let _ = window.set_always_on_top(true);
        let _ = window.show();
    });
}

fn hide_pet_window(app: &AppHandle) {
    if current_mode() == Mode::Card {
        return;
    }
    if let Some(window) = app.get_webview_window(NOTIFY_WINDOW) {
        let _ = window.hide();
    }
    set_mode(app, Mode::Hidden);
}

/// The window's own `CloseRequested` (Alt+F4 on the card, a WM that reclaims a
/// hidden window): hide it and treat the face as gone. Nothing is destroyed —
/// the next show re-sizes and re-places it.
pub fn on_companion_closed(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(NOTIFY_WINDOW) {
        let _ = window.hide();
    }
    set_mode(app, Mode::Hidden);
}

/// Show or hide the pet so the screen matches the config and the main window's
/// focus. A card on screen is never touched: it owns the window until it
/// dismisses, and its dismissal runs this again.
///
/// Called by the prewarm task, by `apply_config` (the Settings switch) and by
/// the main window's focus handler.
pub fn sync_pet(app: &AppHandle) {
    if current_mode() == Mode::Card {
        return;
    }
    let settings = pet_settings(app);
    if pet_allowed(app, &settings) {
        show_pet_window(app, &settings);
    } else {
        hide_pet_window(app);
    }
}

/// The main window gained or lost the focus (`lib.rs`'s window event handler).
///
/// Gaining it keeps the old contract — the card belongs to a background moment
/// — and hides the pet unless the user pinned it on screen. Losing it is when
/// the pet appears: it is the companion for the moments the app is not the
/// foreground window.
pub fn on_main_focus(app: &AppHandle, focused: bool) {
    if focused && current_mode() == Mode::Card {
        dismiss_card(app);
    }
    sync_pet(app);
}

/// A `Moved` event on the companion window: remember a user drag as the pet's
/// anchor, debounced.
pub fn on_pet_moved(app: &AppHandle, position: tauri::PhysicalPosition<i32>) {
    if current_mode() != Mode::Pet {
        return;
    }
    let Some(window) = app.get_webview_window(NOTIFY_WINDOW) else {
        return;
    };
    if !window.is_visible().unwrap_or(false) {
        return;
    }
    let Ok(size) = window.outer_size() else {
        return;
    };
    let anchor = (
        position.x + size.width as i32,
        position.y + size.height as i32,
    );
    // Programmatic placement lands exactly on the stored corner: only a real
    // drag moves the bottom-right away from it, so a matching corner is not a
    // change worth persisting.
    let current = pet_settings(app).anchor;
    if (current.0 - anchor.0).abs() <= 1 && (current.1 - anchor.1).abs() <= 1 {
        return;
    }
    let seq = ANCHOR_SEQ.fetch_add(1, Ordering::SeqCst);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(ANCHOR_SAVE_MS)).await;
        if ANCHOR_SEQ.load(Ordering::SeqCst) != seq {
            return;
        }
        persist_pet_anchor(&app, anchor);
    });
}

/// Write the dragged anchor into the config.
///
/// Deliberately not `apply_config`: a drag must not re-resolve the workspace or
/// re-emit the whole config to the UI. This is the one write path that only
/// touches the pet's own field.
fn persist_pet_anchor(app: &AppHandle, anchor: (i32, i32)) {
    let Some(state) = app.try_state::<crate::AppState>() else {
        return;
    };
    let snapshot = {
        let mut config = state.config.lock().unwrap();
        if config.desktop_pet_anchor == Some([anchor.0, anchor.1]) {
            return;
        }
        config.desktop_pet_anchor = Some([anchor.0, anchor.1]);
        config.clone()
    };
    if let Ok(json) = serde_json::to_string_pretty(&snapshot) {
        let _ = state.instance.write_config(&state.config_path, &json);
    }
}

/// Dismiss the card: back to the pet when it is enabled and allowed, otherwise
/// hide the window.
fn dismiss_card(app: &AppHandle) {
    let settings = pet_settings(app);
    if pet_allowed(app, &settings) {
        show_pet_window(app, &settings);
        return;
    }
    if let Some(window) = app.get_webview_window(NOTIFY_WINDOW) {
        let _ = window.hide();
    }
    set_mode(app, Mode::Hidden);
}

/// Clear the native material and the document attribute, so the window is a
/// plain transparent square again (the pet face).
fn clear_notify_backdrop(app: &AppHandle) -> Option<String> {
    let window = app.get_webview_window(NOTIFY_WINDOW)?;
    // `off` is the same switch the glass level uses: on Windows it clears both
    // Acrylic and Mica, on the other platforms this is the no-op stub.
    let _ = crate::apply_window_backdrop(&window, "off", false);
    #[cfg(target_os = "macos")]
    {
        let _ = window_vibrancy::clear_vibrancy(&window);
    }
    let _ = window.eval(
        "document.documentElement.removeAttribute('data-vibrancy');\
         document.documentElement.removeAttribute('data-glass-fallback');",
    );
    None
}

/// Start the window drag for the pet (`startDragging`), exposed as a command so
/// the pet capability needs no window permission of its own.
pub fn pet_begin_drag(window: WebviewWindow) {
    if current_mode() != Mode::Pet {
        return;
    }
    let _ = window.start_dragging();
}

/// Forward one mood to the pet window. A missing window is normal (the pet is
/// off) and must never create one just to show a mood.
pub fn set_companion_state(app: &AppHandle, state: String) {
    let _ = app.emit_to(
        NOTIFY_WINDOW,
        crate::events::COMPANION_STATE,
        crate::events::CompanionStatePayload { state },
    );
}

/// Global-physical bottom-right corner of the usable area, plus any extra
/// margin the platform shell needs at the bottom.
///
/// Windows asks the nearest monitor of the main window for its real work area
/// (taskbar excluded) through `GetMonitorInfoW`; macOS / Linux get the full
/// monitor bounds and shave a fixed band for the dock / panel.
#[cfg(target_os = "windows")]
fn work_area_bottom_right(app: &AppHandle) -> Result<(i32, i32, i32), String> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };

    // Tauri's `HWND` may be a different windows-sys revision's type; bridge
    // through `usize` so the cast works for both the isize and the pointer
    // spelling.
    let hwnd = app
        .get_webview_window("main")
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as usize as *mut core::ffi::c_void)
        .ok_or_else(|| "main window handle unavailable".to_string())?;
    let hwnd: HWND = hwnd;
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return Err("no monitor for the main window".to_string());
    }
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return Err("GetMonitorInfoW failed".to_string());
    }
    Ok((info.rcWork.right, info.rcWork.bottom, 0))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn work_area_bottom_right(app: &AppHandle) -> Result<(i32, i32, i32), String> {
    let monitor = app
        .get_webview_window("main")
        .and_then(|w| w.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())
        .ok_or_else(|| "no monitor information".to_string())?;
    let pos = monitor.position();
    let size = monitor.size();
    Ok((
        pos.x + size.width as i32,
        pos.y + size.height as i32,
        SHELL_MARGIN_BOTTOM,
    ))
}
