//! The desktop notification card — a second, frameless webview window.
//!
//! When a chat turn settles (done or failed) while the main window is
//! minimized or not focused, the frontend calls [`show_notify_card`]: this
//! module places the pre-created `notify` window at the bottom-right corner of
//! the screen, pushes the title / body to its webview and shows it. The card
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
//! Behaviour contract:
//! - the window never steals focus on show (`focus: false` + a hidden show)
//! - it is hidden again by its own 6 s timer (hover pauses it), by a click
//!   (which also focuses the main window), or when the main window gains
//!   focus (`Focused(true)` in `lib.rs`'s window event handler)
//! - its `CloseRequested` is intercepted — the window must survive app-lifetime

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

pub const NOTIFY_WINDOW: &str = "notify";
pub const NOTIFY_SHOW_EVENT: &str = "notify-show";

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

/// Show the notification card with `title` / `body`.
///
/// `kind` is `"done"` or `"error"` — the card UI colours the title from it
/// (semantic status only, per DESIGN.md).
#[tauri::command]
pub fn show_notify_card(
    app: AppHandle,
    title: String,
    body: String,
    kind: String,
) -> Result<(), String> {
    let Some(window) = app.get_webview_window(NOTIFY_WINDOW) else {
        return Err("notify window is not running".to_string());
    };
    if let Err(err) = position_bottom_right(&app, &window) {
        // A stale position is better than no notification; leave the window
        // wherever it was (first run: the config's default position).
        eprintln!("notify: placement failed: {err}");
    }
    let _ = window.set_always_on_top(true);
    app.emit_to(NOTIFY_WINDOW, NOTIFY_SHOW_EVENT, NotifyPayload { title, body, kind })
        .map_err(|e| e.to_string())?;
    // The card webview listens for `notify-show` to render; the window itself
    // is only the canvas. `show()` never activates it (tao uses
    // SW_SHOWNOACTIVATE, and the config carries `focus: false`), so the user's
    // current application keeps the foreground.
    window.show().map_err(|e| e.to_string())?;

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
        if let Some(window) = handle.get_webview_window(NOTIFY_WINDOW) {
            if window.is_visible().unwrap_or(false) {
                let _ = window.hide();
            }
        }
    });
    Ok(())
}

/// Hide the card. Called by the card UI itself (auto-dismiss timer).
#[tauri::command]
pub fn hide_notify_card(window: WebviewWindow) {
    let _ = window.hide();
}

/// Clicking the card hides it and brings the main window back, un-minimized
/// and focused — the toast's one interaction, mirroring a system toast click.
#[tauri::command]
pub fn focus_main_from_notify(app: AppHandle, window: WebviewWindow) {
    let _ = window.hide();
    crate::focus_main_window(&app);
}

/// The card window asks for the current backdrop state on mount and before
/// every show, instead of being pushed one: an `eval` from `setup` can race
/// the webview load and the attribute would be silently lost. Returns the
/// backdrop kind in effect (`mica` / `acrylic` / `vibrancy`) or `None` —
/// `None` means the card paints its own fallback wash.
#[tauri::command]
pub fn refresh_notify_backdrop(app: AppHandle) -> Option<String> {
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
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        if level == "off" {
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

/// Place the card at the bottom-right of the area it may occupy, so it reads
/// like the system's own toast position.
fn position_bottom_right(app: &AppHandle, window: &WebviewWindow) -> Result<(), String> {
    let (right, bottom, extra_bottom) = work_area_bottom_right(app)?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let w = (CARD_WIDTH * scale).round() as i32;
    let h = (CARD_HEIGHT * scale).round() as i32;
    let x = (right - MARGIN_RIGHT - w).max(0);
    let y = (bottom - extra_bottom - MARGIN_BOTTOM - h).max(0);
    window
        .set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())
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
