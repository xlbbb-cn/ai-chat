//! Forward fallbacks for a font that carries **no** `ToUnicode` CMap.
//!
//! A missing `ToUnicode` does not mean the bytes are unknowable. Two routes are
//! exact, not guesses:
//!
//! 1. **Predefined CMap names.** A composite font may declare `/Encoding
//!    /GBK-EUC-H` or `/Encoding /UniGB-UCS2-H` instead of a CMap stream, and the
//!    PDF specification fixes what those mean: with `GBK-EUC-H` the character
//!    codes *are* GBK byte sequences, and with `Uni*-UCS2` / `Uni*-UTF16` they
//!    *are* UTF-16 code units written straight into the string. The generator
//!    never writes a mapping because the mapping is the name.
//! 2. **CID ordering.** With `/Encoding /Identity-H` and a descendant whose
//!    `CIDSystemInfo` says `/Ordering (UCS)` (or `(Unicode)`), the CID *is* the
//!    UCS-2 value — again by definition, not by inference.
//!
//! Only when both routes come up empty do we get to guess: try the CJK byte
//! encodings, keep the one that produces text that could not plausibly be
//! anything else, and let the font name decide how much evidence is enough. A
//! wrong guess that reads as Chinese is worse than today's Latin-1 output,
//! because it looks right — so an unhinted font name must clear a gate of
//! *nothing but* CJK, and is not even offered UTF-16 (arbitrary two-byte glyph
//! ids pair up into ideographs disturbingly often).
//!
//! `encoding_rs` carries the WHATWG byte tables, which match Adobe's GBK /
//! Big5 / Shift_JIS / EUC-KR collections closely enough to read real documents
//! (they are the same national standards the CMaps were built from), at a
//! fraction of the size of shipping the CMap files themselves.

use Charset::*;

/// A byte encoding we can turn into Unicode without a per-document table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Charset {
    /// Two-byte big-endian code units — `Uni*-UCS2`, `Uni*-UTF16`, UCS ordering.
    Utf16Be,
    /// Three-byte `Uni*-UTF8` codes.
    Utf8,
    /// `GBK-EUC-*`, `GB*-EUC-*`, `GBK*` — GBK / EUC-CN.
    Gbk,
    /// `Big5-*`, `CNS-*`, `ETen-*`, `Bs-Big5-*`, `Uni*-Big5*`.
    Big5,
    /// `83pv-RKSJ-*`, `90ms-RKSJ-*`, `JIS*` — Shift_JIS.
    ShiftJis,
    /// `EUC-*`, `EUCpv-*`, `UniJIS-EUC*` — EUC-JP.
    EucJp,
    /// `KSC*`, `KS-*` — EUC-KR.
    EucKr,
}

/// The multi-byte CJK byte encodings, in the order a guess tries them when the
/// name says nothing. `Utf16Be` is deliberately absent; see the module docs.
const BYTE_CANDIDATES: [Charset; 5] = [Gbk, Big5, ShiftJis, EucJp, EucKr];

impl Charset {
    /// Decode a string operand. Never fails: bytes that do not map become
    /// U+FFFD, which [`Charset::plausible`] then refuses.
    pub fn decode(self, bytes: &[u8]) -> String {
        match self {
            Utf16Be => {
                let units: Vec<u16> = bytes
                    .chunks(2)
                    .filter(|pair| pair.len() == 2)
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                    .collect();
                char::decode_utf16(units)
                    .filter_map(|ch| ch.ok())
                    .filter(|ch| *ch != '\u{FEFF}')
                    .collect()
            }
            Utf8 => String::from_utf8_lossy(bytes).into_owned(),
            Gbk => encoding_rs::GBK.decode(bytes).0.into_owned(),
            Big5 => encoding_rs::BIG5.decode(bytes).0.into_owned(),
            ShiftJis => encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned(),
            EucJp => encoding_rs::EUC_JP.decode(bytes).0.into_owned(),
            EucKr => encoding_rs::EUC_KR.decode(bytes).0.into_owned(),
        }
    }

    /// The charset a predefined CMap name guarantees, `None` for names we do not
    /// know and for `Identity-*` (whose meaning depends on the CID ordering).
    pub fn from_cmap_name(name: &[u8]) -> Option<Charset> {
        let upper = String::from_utf8_lossy(name).to_ascii_uppercase();
        let stem = upper.split('-').next().unwrap_or(upper.as_str());
        // `UniGB-UCS2-H`, `UniJIS-UTF16-H`, `UniKS-UTF8-EUC-KR`…: the character
        // code is the Unicode value itself, whatever the CIDs of the font are.
        if stem.starts_with("UNI") {
            if upper.contains("UCS2") || upper.contains("UTF16") {
                return Some(Utf16Be);
            }
            if upper.contains("UTF8") {
                return Some(Utf8);
            }
        }
        if upper.contains("UCS2") || upper.contains("UTF16") {
            return Some(Utf16Be);
        }
        if upper.contains("GBK") || upper.contains("GB-EUC") || upper.contains("GBPC") {
            return Some(Gbk);
        }
        if upper.contains("BIG5")
            || upper.contains("B5")
            || upper.contains("CNS")
            || upper.contains("ETEN")
            || upper.contains("CP950")
        {
            return Some(Big5);
        }
        if upper.contains("RKSJ") || upper.contains("JIS") || upper.contains("SJIS") {
            return Some(ShiftJis);
        }
        if upper.starts_with("EUC") || upper.contains("EUC-JP") {
            return Some(EucJp);
        }
        if upper.contains("KSC") || upper.starts_with("KS-") || upper.contains("KS-EUC") {
            return Some(EucKr);
        }
        None
    }

    /// `Identity-*` codes are only Unicode when the descendant says so.
    ///
    /// `/CIDSystemInfo` with `/Ordering (UCS)` or `(Unicode)` means the CID is
    /// the UCS-2 code, so the two bytes in the operand can be read directly.
    /// Adobe's national orderings (`GB1`, `CNS1`, `Japan1`, `Korea1`) number
    /// glyphs instead, which no amount of reading the PDF dictionary can map.
    pub fn from_cid_ordering(ordering: &[u8]) -> Option<Charset> {
        let name = String::from_utf8_lossy(ordering).to_ascii_uppercase();
        match name.as_str() {
            "UCS" | "UNICODE" | "UCS2" => Some(Utf16Be),
            _ => None,
        }
    }

    /// The charsets to guess with, best first for the family `font_name` names.
    ///
    /// The families are checked in this order on purpose: the specific ones
    /// first, so `HeiseiMin` is not mistaken for a `…hei` Simplified face and
    /// `PMingLiU` is not mistaken for `…ming`.
    pub fn guessed_for_name(font_name: &str) -> Vec<Charset> {
        let lower = font_name.to_ascii_lowercase();
        let preferred: &[Charset] = if contains_any(
            &lower,
            &["batang", "gulim", "malgun", "hgs", "hys", "myungjo", "ks-"],
        ) {
            &[EucKr]
        } else if contains_any(
            &lower,
            &[
                "heisei", "mincho", "gothic", "hiragino", "meiryo", "jis", "saisei", "ipa",
                "yugothic", "yuminn",
            ],
        ) {
            &[ShiftJis, EucJp]
        } else if contains_any(
            &lower,
            &[
                "msung", "pming", "mingliu", "big5", "cns", "b5", "kai-u", "mmph", "tsang",
                "hant",
            ],
        ) {
            &[Big5]
        } else if contains_any(
            &lower,
            &[
                "simsun", "nsimsun", "songti", "yahei", "simhei", "kaiti", "fangsong", "youyuan",
                "simyou", "dengxian", "gb2312", "hans", "sourcehan", "notocjk", "notosanscjk",
                "notoserifcjk", "hei", "kai", "song", "sun",
            ],
        ) {
            &[Gbk]
        } else {
            &[]
        };
        let mut ordered: Vec<Charset> = preferred.to_vec();
        if Self::looks_cjk(font_name) {
            // A CJK name is enough evidence to also try reading the codes as
            // UTF-16: that is what a `UCS`-ordered subset without a documented
            // ordering looks like, and it is the cheapest way to recover one.
            ordered.push(Utf16Be);
        }
        for charset in BYTE_CANDIDATES {
            if !ordered.contains(&charset) {
                ordered.push(charset);
            }
        }
        ordered
    }

    /// `true` when a font name belongs to a CJK family — the only case in which
    /// the weaker scoring gate and the UTF-16 candidate are open.
    pub fn looks_cjk(font_name: &str) -> bool {
        let lower = font_name.to_ascii_lowercase();
        contains_any(
            &lower,
            &[
                "cjk", "han", "hans", "hant", "hanzi", "sim", "song", "sun", "hei", "kai",
                "fang", "deng", "ming", "gothic", "mincho", "heisei", "batang", "gulim",
                "malgun", "msung", "yahei", "gb2312", "big5", "jis", "ksc", "adobegb",
                "sourcehan", "stkai", "stsong", "sthei", "libian", "youyuan", "unicode",
            ],
        )
    }

    /// Should `text` be believed as the decoding of `bytes`?
    ///
    /// A decoding that contains a replacement character or a control character
    /// is always refused: those are exactly what a wrong byte offset produces.
    /// `relaxed` (only for a font whose own name claims to be CJK) accepts a
    /// 40 % CJK share, which tolerates the odd Latin digit or bracket inside
    /// real Chinese prose. Without a hint the run must be **nothing but** CJK —
    /// a Latin string that happens to pair into code points is the failure this
    /// gate exists to stop.
    pub fn plausible(self, bytes: &[u8], text: &str, relaxed: bool) -> bool {
        if text.is_empty() {
            return false;
        }
        match self {
            Utf16Be if bytes.len() % 2 != 0 => return false,
            Utf8 if std::str::from_utf8(bytes).is_err() => return false,
            _ => {}
        }
        let mut meaningful = 0usize;
        let mut cjk = 0usize;
        for ch in text.chars() {
            if ch.is_whitespace() {
                continue;
            }
            if ch == '\u{FFFD}' || (ch.is_control() && !matches!(ch, '\t' | '\n' | '\r')) {
                return false;
            }
            meaningful += 1;
            if is_cjk(ch) {
                cjk += 1;
            }
        }
        if meaningful == 0 {
            // Spaces and nothing else: every charset agrees, so let the caller
            // use whichever it already settled on.
            return true;
        }
        let share = cjk as f32 / meaningful as f32;
        if relaxed {
            share >= 0.4
        } else {
            share >= 1.0
        }
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

/// CJK ideographs, kana, hangul, and the punctuation and full-width forms that
/// ride with them.
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x2E80..=0x2FFF    // radicals, Kangxi, CJK symbols
        | 0x3000..=0x303F  // CJK punctuation
        | 0x3040..=0x30FF  // kana
        | 0x3100..=0x312F  // bopomofo
        | 0x31A0..=0x31BF
        | 0x3200..=0x33FF  // enclosed CJK
        | 0x3400..=0x4DBF  // ideograph extension A
        | 0x4E00..=0x9FFF  // ideographs
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3  // hangul syllables
        | 0xF900..=0xFAFF  // compatibility ideographs
        | 0xFE10..=0xFE4F  // vertical forms
        | 0xFF00..=0xFF60  // full-width forms
        | 0x20000..=0x2FA1F
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predefined_names_that_are_their_own_mapping() {
        assert_eq!(Charset::from_cmap_name(b"UniGB-UCS2-H"), Some(Utf16Be));
        assert_eq!(Charset::from_cmap_name(b"UniCNS-UCS2-V"), Some(Utf16Be));
        assert_eq!(Charset::from_cmap_name(b"UniJIS-UTF16-H"), Some(Utf16Be));
        assert_eq!(Charset::from_cmap_name(b"UniKS-UTF8-H"), Some(Utf8));
        assert_eq!(Charset::from_cmap_name(b"GBK-EUC-H"), Some(Gbk));
        assert_eq!(Charset::from_cmap_name(b"gbk2k-v"), Some(Gbk));
        assert_eq!(Charset::from_cmap_name(b"GBpc-EUC-H"), Some(Gbk));
        assert_eq!(Charset::from_cmap_name(b"Big5-H"), Some(Big5));
        assert_eq!(Charset::from_cmap_name(b"CNS-EUC-V"), Some(Big5));
        assert_eq!(Charset::from_cmap_name(b"ETen-B5-H"), Some(Big5));
        assert_eq!(Charset::from_cmap_name(b"90ms-RKSJ-H"), Some(ShiftJis));
        assert_eq!(Charset::from_cmap_name(b"EUC-H"), Some(EucJp));
        assert_eq!(Charset::from_cmap_name(b"KSCms-UHC-H"), Some(EucKr));
        // Identity is not a charset by itself, and neither is an unknown name.
        assert_eq!(Charset::from_cmap_name(b"Identity-H"), None);
        assert_eq!(Charset::from_cmap_name(b"WinAnsiEncoding"), None);
        assert_eq!(Charset::from_cmap_name(b"MacRomanEncoding"), None);
        assert_eq!(Charset::from_cmap_name(b""), None);
    }

    #[test]
    fn ucs_ordering_makes_identity_codes_unicode() {
        assert_eq!(Charset::from_cid_ordering(b"UCS"), Some(Utf16Be));
        assert_eq!(Charset::from_cid_ordering(b"ucs"), Some(Utf16Be));
        assert_eq!(Charset::from_cid_ordering(b"GB1"), None);
        assert_eq!(Charset::from_cid_ordering(b"Japan1"), None);
    }

    #[test]
    fn utf16be_reads_the_codes_directly() {
        // 目 is U+76EE and 录 U+5F55: the operand already holds Unicode.
        assert_eq!(Utf16Be.decode(&[0x76, 0xEE, 0x5F, 0x55]), "目录");
        assert_eq!(Utf16Be.decode(&[0xFE, 0xFF, 0x76, 0xEE]), "目");
        // An odd length cannot be UTF-16, and one ASCII character is not proof.
        assert_eq!(Utf16Be.decode(&[0x00, 0x41, 0x00]), "A");
        assert!(!Utf16Be.plausible(&[0x00, 0x41, 0x00], "A", false));
    }

    #[test]
    fn gbk_and_big5_come_from_the_byte_tables() {
        // 目录 in GBK is C4 BF C2 BC.
        assert_eq!(Gbk.decode(&[0xC4, 0xBF, 0xC2, 0xBC]), "目录");
        assert!(Gbk.plausible(&[0xC4, 0xBF, 0xC2, 0xBC], "目录", false));
        // 台語 in Big5 is A5 78 BB 79.
        let big5 = Big5.decode(&[0xA5, 0x78, 0xBB, 0x79]);
        assert_eq!(big5, "台語");
        assert!(Big5.plausible(&[0xA5, 0x78, 0xBB, 0x79], &big5, false));
        // 日本語 in Shift_JIS is 93 FA 96 7B 8C EA.
        let sjis = ShiftJis.decode(&[0x93, 0xFA, 0x96, 0x7B, 0x8C, 0xEA]);
        assert_eq!(sjis, "日本語");
    }

    #[test]
    fn a_latin_run_is_never_rescued_by_a_guess() {
        // Even length, so UTF-16 pairing is not refused on structure alone: the
        // CJK-share gate has to be what stops it.
        let bytes = b"Hello world, this is prose!!";
        assert_eq!(bytes.len() % 2, 0);
        for charset in BYTE_CANDIDATES {
            let text = charset.decode(bytes);
            assert!(
                !charset.plausible(bytes, &text, false),
                "{charset:?} should refuse {text:?}"
            );
        }
        // The unhinted candidate list does not even offer UTF-16, which is what
        // pairs this prose into ideographs.
        let guessed = Charset::guessed_for_name("Helvetica");
        assert!(!guessed.contains(&Utf16Be), "{guessed:?}");
        assert!(!Utf16Be.plausible(bytes, &Utf16Be.decode(bytes), false));
    }

    #[test]
    fn replacement_and_control_characters_refuse_a_candidate() {
        assert!(!Gbk.plausible(&[0x81, 0x40], "\u{FFFD}", false));
        assert!(!Utf16Be.plausible(&[0x00, 0x07], "\u{7}", false));
        assert!(!Big5.plausible(&[0xA5, 0x78, 0x00, 0x01], "台\u{1}", false));
    }

    #[test]
    fn spaces_only_are_always_acceptable() {
        assert!(Gbk.plausible(b"   ", "   ", false));
    }

    #[test]
    fn name_hints_order_the_candidates() {
        let simsun = Charset::guessed_for_name("AAAAAA+SimSun");
        assert_eq!(simsun.first(), Some(&Gbk));
        assert_eq!(simsun.len(), 6, "{simsun:?}");
        assert!(Charset::looks_cjk("AAAAAA+MicrosoftYaHei-Bold"));

        let msung = Charset::guessed_for_name("AAAAAB+MSung-Light");
        assert_eq!(msung.first(), Some(&Big5));

        let heisei = Charset::guessed_for_name("HeiseiMin-W3");
        assert_eq!(heisei.first(), Some(&ShiftJis));

        let batang = Charset::guessed_for_name("Batang");
        assert_eq!(batang.first(), Some(&EucKr));

        // An unremarkable name gets the byte encodings only, no duplicates.
        let helvetica = Charset::guessed_for_name("Helvetica-Bold");
        assert!(!Charset::looks_cjk("Times New Roman"));
        assert_eq!(helvetica.len(), 5, "{helvetica:?}");
        assert_eq!(
            helvetica.iter().collect::<std::collections::HashSet<_>>().len(),
            5
        );
    }

    #[test]
    fn relaxed_gate_needs_half_a_majority_and_strict_needs_all() {
        // Two ideographs and three ASCII letters: 40 % — enough for a CJK-named
        // font, nowhere near enough for an anonymous one.
        let text = "中文abc";
        let bytes = [0xD6, 0xD0, 0xCE, 0xC4, 0x61, 0x62, 0x63];
        assert!(Gbk.plausible(&bytes, text, true));
        assert!(!Gbk.plausible(&bytes, text, false));
        // All-CJK passes either way.
        assert!(Gbk.plausible(&bytes[..4], "中文", false));
    }
}
