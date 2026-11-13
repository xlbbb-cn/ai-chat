// The OS user-interface language, read once at startup.
//
// The frontend can only see `navigator.language`, which on Windows is WebView2's
// preferred-language list — not necessarily the Windows UI language. The value
// read here is written into `config.json` on the first launch (see `setup` in
// lib.rs), so the UI opens in a language the user can read and a later change
// of the system language cannot silently switch the app again.
//
// The mapping mirrors `detectSystemLocale()` in `src/i18n/index.tsx`: any
// Chinese tag with a Taiwan / Hong Kong / Macau / `Hant` marker is Traditional,
// every other Chinese tag Simplified, everything else English.

/// Locales bundled by the frontend (`src/i18n/index.tsx`, `LOCALES`).
const SUPPORTED: [&str; 3] = ["en", "zh-CN", "zh-TW"];

/// Is `value` one of the bundled locales?
///
/// A config carrying anything else (older build, hand edit) counts as "never
/// chosen" and gets claimed again on the next start.
pub fn is_supported(value: &str) -> bool {
    SUPPORTED.contains(&value)
}

/// Best-effort match of the OS user-interface language to a bundled locale.
///
/// `None` means the platform reads nothing (no locale environment variable on
/// Linux, `defaults` unavailable on macOS): the caller leaves `language`
/// unset and the frontend's own detection decides.
pub fn detect() -> Option<&'static str> {
    from_tag(&platform_tag()?)
}

/// Map a locale tag (`zh_CN`, `zh-Hant-TW`, `de-DE`, …) to a bundled locale.
///
/// Unknown languages land on English: it is the fallback every message key
/// already falls back to, so pinning it is the honest outcome.
fn from_tag(tag: &str) -> Option<&'static str> {
    let tag = tag.trim().to_ascii_lowercase();
    if tag.is_empty() {
        return None;
    }
    if tag.starts_with("zh") {
        // Traditional is used in Taiwan, Hong Kong and Macau; `Hant` says so
        // explicitly (e.g. `zh-Hant` without a region).
        return Some(
            if ["hant", "tw", "hk", "mo"]
                .iter()
                .any(|marker| tag.contains(marker))
            {
                "zh-TW"
            } else {
                "zh-CN"
            },
        );
    }
    Some("en")
}

/// The raw tag, per platform.
#[cfg(windows)]
fn platform_tag() -> Option<String> {
    use windows_sys::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};

    // LOCALE_NAME_MAX_LENGTH. The call reports the length including the NUL.
    let mut buffer = [0u16; 85];
    // The user's UI language (what Explorer and Settings themselves use), not
    // the formatting locale: a config may prefer en-US while the UI is Chinese.
    let langid = unsafe { GetUserDefaultUILanguage() };
    // A LANGID is an LCID with the default sort order.
    let len = unsafe {
        LCIDToLocaleName(
            langid as u32,
            buffer.as_mut_ptr(),
            buffer.len() as i32,
            0,
        )
    };
    if len <= 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..len as usize - 1]))
}

#[cfg(target_os = "macos")]
fn platform_tag() -> Option<String> {
    // No Foundation binding in this build and this runs at most once per
    // install, so read the global preference through the system tool.
    let output = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(target_os = "linux")]
fn platform_tag() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        // `zh_CN.UTF-8` -> `zh_CN`; the codeset never carries the language.
        .map(|value| value.split('.').next().unwrap_or_default().to_string())
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn platform_tag() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_tags_split_on_script_and_region() {
        assert_eq!(from_tag("zh-CN"), Some("zh-CN"));
        assert_eq!(from_tag("zh_Hans_CN"), Some("zh-CN"));
        assert_eq!(from_tag("zh-SG"), Some("zh-CN"));
        assert_eq!(from_tag("zh"), Some("zh-CN"));
        assert_eq!(from_tag("zh-TW"), Some("zh-TW"));
        assert_eq!(from_tag("zh-HK"), Some("zh-TW"));
        assert_eq!(from_tag("zh_MO"), Some("zh-TW"));
        assert_eq!(from_tag("zh-Hant"), Some("zh-TW"));
        assert_eq!(from_tag("zh-hant-tw"), Some("zh-TW"));
    }

    #[test]
    fn other_languages_pin_english() {
        assert_eq!(from_tag("en-US"), Some("en"));
        assert_eq!(from_tag("de-DE"), Some("en"));
        assert_eq!(from_tag("ja_JP.UTF-8"), Some("en"));
        assert_eq!(from_tag("  EN-gb  "), Some("en"));
    }

    #[test]
    fn nothing_to_read_stays_unset() {
        assert_eq!(from_tag(""), None);
        assert_eq!(from_tag("   "), None);
    }

    #[test]
    fn only_bundled_locales_are_supported() {
        assert!(is_supported("en"));
        assert!(is_supported("zh-CN"));
        assert!(is_supported("zh-TW"));
        assert!(!is_supported("zh-HK"));
        assert!(!is_supported("EN"));
        assert!(!is_supported(""));
    }

    /// Whatever this machine reports, it must be something the frontend has.
    #[test]
    fn detection_matches_a_bundled_locale() {
        if let Some(locale) = detect() {
            assert!(is_supported(locale), "unexpected locale '{locale}'");
        }
    }
}
