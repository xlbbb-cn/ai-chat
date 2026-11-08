//! A tolerant `ToUnicode` CMap reader.
//!
//! `lopdf` does parse these CMaps, but with a strict `nom` grammar that walks the
//! whole file: `/CIDInit … begin  <n> dict begin  begincmap  <metadata>  …`. If any
//! token of the *header* is off spec, the parser returns an error, and
//! [`lopdf::Dictionary::get_font_encoding`] swallows it behind a `log::warn` and
//! hands back `StandardEncoding` instead. Every two-byte CID of the page is then
//! decoded as two one-byte codes — visible mojibake rather than a hole, but
//! useless text.
//!
//! Real generators hit that path constantly. The book this crate was calibrated
//! on (`学会生存`) writes its CMap header as
//!
//! ```postscript
//! /CMapName SimSun-cmap def      %% the PostScript name has no leading slash
//! ```
//!
//! which is invalid PostScript but perfectly readable for a human, and one
//! missing `/` cost the entire 174-page book its Chinese. So the extractor owns
//! its own parser here and treats the header as noise: it scans for
//! `codespacerange` / `bfchar` / `bfrange` sections and ignores everything else.
//!
//! Destinations are read as big-endian UTF-16 code units, so a surrogate pair
//! (`<00D83DD3>`) becomes one astral character.

use std::collections::HashMap;

/// Largest `bfrange` we are willing to expand into individual entries. A
/// pathological `<0000> <FFFFFF> <0000>` line must not allocate for days.
const MAX_RANGE_ENTRIES: usize = 1 << 16;

/// One `ToUnicode` CMap: character code (with its byte length) to UTF-16 units.
#[derive(Debug, Clone, Default)]
pub struct UnicodeMap {
    /// Entries bucketed by code length, index 1..=4.
    by_len: [HashMap<u32, Vec<u16>>; 5],
    /// Shortest and longest code the CMap declares, from `begincodespacerange`
    /// when present and from the mapping entries otherwise.
    min_len: usize,
    max_len: usize,
    /// Number of mapped codes.
    mapped: usize,
}

/// What decoding one string operand produced.
#[derive(Debug, Clone, Default)]
pub struct Decoded {
    pub text: String,
    /// Codes the operand was cut into.
    pub codes: usize,
    /// Codes the CMap does not cover.
    pub unmapped: usize,
}

impl Decoded {
    /// Codes that did map.
    pub fn mapped(&self) -> usize {
        self.codes - self.unmapped
    }
}

impl UnicodeMap {
    /// Parse a CMap program, tolerating whatever the generator wrote around the
    /// mapping sections. `None` when no usable mapping is found at all.
    pub fn parse(content: &[u8]) -> Option<UnicodeMap> {
        let mut map = UnicodeMap {
            min_len: usize::MAX,
            max_len: 0,
            ..Default::default()
        };
        let mut scanner = Scanner::new(content);
        // The codespace ranges only tell us the code *lengths*; remember the
        // narrowest and widest so decoding does not have to guess.
        let mut declared_lengths: Vec<usize> = Vec::new();

        while let Some(token) = scanner.next() {
            match token {
                Token::Word(word) if word == b"beginbfchar".as_slice() => {
                    let body = scanner.until(b"endbfchar");
                    map.read_bfchar(&body);
                }
                Token::Word(word) if word == b"beginbfrange".as_slice() => {
                    let body = scanner.until(b"endbfrange");
                    map.read_bfrange(&body);
                }
                Token::Word(word) if word == b"begincodespacerange".as_slice() => {
                    let body = scanner.until(b"endcodespacerange");
                    let mut lengths = Scanner::code_lengths(&body);
                    declared_lengths.append(&mut lengths);
                }
                _ => {}
            }
        }

        if map.mapped == 0 {
            return None;
        }
        if !declared_lengths.is_empty() {
            map.min_len = *declared_lengths.iter().min().unwrap_or(&1);
            map.max_len = *declared_lengths.iter().max().unwrap_or(&1);
        }
        map.trim_lengths();
        Some(map)
    }

    /// `true` when the CMap maps nothing.
    pub fn is_empty(&self) -> bool {
        self.mapped == 0
    }

    /// Cut `bytes` into codes and translate them.
    ///
    /// Longest code first, as the specification requires (a two-byte `<0041>`
    /// must not be matched as two one-byte `<00>` `<41>`). An unmapped code
    /// consumes `min_len` bytes, which keeps a fixed-width code space aligned
    /// instead of letting one hole shift the whole run sideways.
    pub fn decode(&self, bytes: &[u8]) -> Decoded {
        let mut out = Decoded::default();
        if self.is_empty() {
            return out;
        }
        let step = self.min_len.max(1);
        let mut index = 0usize;
        while index < bytes.len() {
            let mut hit = None;
            let widest = self.max_len.min(bytes.len() - index);
            let mut length = widest;
            while length >= self.min_len && length >= 1 {
                let code = read_code(&bytes[index..index + length]);
                if let Some(dst) = self.by_len[length].get(&code) {
                    hit = Some((length, dst.clone()));
                    break;
                }
                length -= 1;
            }
            match hit {
                Some((length, dst)) => {
                    out.codes += 1;
                    push_units(&mut out.text, &dst);
                    index += length;
                }
                None => {
                    out.codes += 1;
                    out.unmapped += 1;
                    index += step.min(bytes.len() - index).max(1);
                }
            }
        }
        out
    }

    fn put(&mut self, code: &[u8], dst: &[u8]) {
        let length = code.len().min(4);
        if length == 0 || dst.is_empty() {
            return;
        }
        let mut units = Vec::with_capacity(dst.len() / 2 + 1);
        collect_units(dst, &mut units);
        if units.is_empty() {
            return;
        }
        if self
            .by_len[length]
            .insert(read_code(code), units)
            .is_none()
        {
            self.mapped += 1;
        }
        self.min_len = self.min_len.min(length);
        self.max_len = self.max_len.max(length);
    }

    /// `beginbfchar`: `<src> <dst>` pairs.
    fn read_bfchar(&mut self, body: &[Token]) {
        let mut index = 0usize;
        while index + 1 < body.len() {
            match (&body[index], &body[index + 1]) {
                (Token::Hex(src), Token::Hex(dst)) => {
                    self.put(src, dst);
                    index += 2;
                }
                _ => index += 1, // junk between pairs: resync on the next token
            }
        }
    }

    /// `beginbfrange`: `<lo> <hi> <dst>` or `<lo> <hi> [ <dst> … ]`.
    fn read_bfrange(&mut self, body: &[Token]) {
        let mut index = 0usize;
        while index + 2 < body.len() {
            match (&body[index], &body[index + 1]) {
                (Token::Hex(lo), Token::Hex(hi)) => {
                    let length = lo.len().min(4);
                    let (start, end) = (read_code(lo), read_code(hi));
                    if end < start || length == 0 {
                        index += 1;
                        continue;
                    }
                    match &body[index + 2] {
                        Token::Hex(dst) => {
                            // Uniform increment: the destination is a code that
                            // rises with the source code.
                            let mut units = Vec::new();
                            collect_units(dst, &mut units);
                            if units.len() == 1 {
                                let base = units[0] as u32;
                                let span = (end - start).min(MAX_RANGE_ENTRIES as u32 - 1);
                                for offset in 0..=span {
                                    let code = start + offset;
                                    let dst = base.wrapping_add(offset) as u16;
                                    self.by_len[length]
                                        .insert(code, vec![dst])
                                        .take_or_count(&mut self.mapped);
                                    self.min_len = self.min_len.min(length);
                                    self.max_len = self.max_len.max(length);
                                }
                            } else if units.len() > 1 {
                                // A multi-unit destination with a range is
                                // ill-formed; map the first code only.
                                self.put(lo, dst);
                            }
                            index += 3;
                        }
                        Token::Open => {
                            let mut offset = index + 3;
                            let mut entries: Vec<Vec<u16>> = Vec::new();
                            while offset < body.len() {
                                match &body[offset] {
                                    Token::Close => break,
                                    Token::Hex(dst) => {
                                        let mut units = Vec::new();
                                        collect_units(dst, &mut units);
                                        entries.push(units);
                                    }
                                    _ => {}
                                }
                                offset += 1;
                            }
                            let span = entries.len().min((end - start + 1) as usize);
                            for (i, units) in entries[..span].iter().enumerate() {
                                if units.is_empty() {
                                    continue;
                                }
                                let code = start + i as u32;
                                self.by_len[length]
                                    .insert(code, units.clone())
                                    .take_or_count(&mut self.mapped);
                                self.min_len = self.min_len.min(length);
                                self.max_len = self.max_len.max(length);
                            }
                            index = offset + 1;
                        }
                        _ => index += 1,
                    }
                }
                _ => index += 1,
            }
        }
    }

    /// Keep the code lengths inside the range a byte string can express, and
    /// make sure every length actually used is searchable.
    fn trim_lengths(&mut self) {
        self.min_len = self.min_len.clamp(1, 4);
        self.max_len = self.max_len.clamp(self.min_len, 4);
        let mut shortest = usize::MAX;
        let mut longest = 0usize;
        for length in 1..=4 {
            if !self.by_len[length].is_empty() {
                shortest = shortest.min(length);
                longest = longest.max(length);
            }
        }
        if longest > 0 {
            // A CMap that declares `<0000> <FFFF>` but only maps one-byte codes
            // must not make the decoder swallow two bytes at a time.
            self.min_len = self.min_len.min(shortest);
            self.max_len = longest;
        }
    }
}

/// `Option<V>::insert` returns the previous value; this counts only genuinely
/// new entries so `mapped` stays honest when ranges overlap.
trait TakeOrCount {
    fn take_or_count(self, counter: &mut usize);
}
impl<V> TakeOrCount for Option<V> {
    fn take_or_count(self, counter: &mut usize) {
        if self.is_none() {
            *counter += 1;
        }
    }
}

/// Big-endian read of up to four bytes as a code.
fn read_code(bytes: &[u8]) -> u32 {
    let mut code = 0u32;
    for byte in bytes.iter().take(4) {
        code = (code << 8) | u32::from(*byte);
    }
    code
}

/// Read destination bytes as big-endian UTF-16 units; an odd trailing byte is
/// treated as the low half of one unit (`<41>` means `U+0041`).
fn collect_units(dst: &[u8], units: &mut Vec<u16>) {
    let mut index = 0usize;
    if dst.len() % 2 == 1 {
        units.push(u16::from(dst[0]));
        index = 1;
    }
    while index + 1 < dst.len() {
        units.push(u16::from_be_bytes([dst[index], dst[index + 1]]));
        index += 2;
    }
}

/// Append the units as characters, pairing surrogates and dropping a stray BOM.
fn push_units(out: &mut String, units: &[u16]) {
    for ch in char::decode_utf16(units.iter().copied()) {
        match ch {
            Ok('\u{FEFF}') => {}
            Ok(ch) => out.push(ch),
            // A lone surrogate is a broken destination, not a character: drop it
            // rather than pushing U+FFFD into the text.
            Err(_) => {}
        }
    }
}

/// A token of the CMap program.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// `<…>` as raw bytes (zero-padded on the left when the nibble count is odd).
    Hex(Vec<u8>),
    /// A bare or slash-prefixed word: `beginbfchar`, `def`, `SimSun-cmap`…
    Word(Vec<u8>),
    /// A number.
    Number(Vec<u8>),
    Open,
    Close,
    /// Anything else (`(Adobe)`, `<<`, `>>`, `12`).
    Other,
}

impl Token {
    fn is_keyword(&self, keyword: &[u8]) -> bool {
        matches!(self, Token::Word(word) if word == keyword)
    }
}

/// Minimal CMap tokenizer. Unlike a PDF content stream, a CMap has no strings
/// with escapes to honour, so `(…)` and `<<…>>` are simply skipped over.
struct Scanner<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn new(src: &'a [u8]) -> Self {
        Scanner { src, pos: 0 }
    }

    fn skip_trivia(&mut self) {
        while self.pos < self.src.len() {
            let byte = self.src[self.pos];
            match byte {
                b' ' | b'\t' | b'\r' | b'\n' | 0x0c | 0x0b => self.pos += 1,
                b'%' => {
                    while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                _ => return,
            }
        }
    }

    fn next(&mut self) -> Option<Token> {
        self.skip_trivia();
        if self.pos >= self.src.len() {
            return None;
        }
        let byte = self.src[self.pos];
        match byte {
            b'<' => {
                if self.src[self.pos + 1..].first() == Some(&b'<') {
                    self.pos += 2;
                    self.skip_dictionary();
                    return Some(Token::Other);
                }
                self.pos += 1;
                let start = self.pos;
                while self.pos < self.src.len() && self.src[self.pos] != b'>' {
                    self.pos += 1;
                }
                let raw = &self.src[start..self.pos.min(self.src.len())];
                if self.pos < self.src.len() {
                    self.pos += 1;
                }
                Some(Token::Hex(hex_bytes(raw)))
            }
            b'[' => {
                self.pos += 1;
                Some(Token::Open)
            }
            b']' => {
                self.pos += 1;
                Some(Token::Close)
            }
            b'{' => {
                // Some generators use `{ … }` where the spec says `[ … ]`.
                self.pos += 1;
                Some(Token::Open)
            }
            b'}' => {
                self.pos += 1;
                Some(Token::Close)
            }
            b'(' => {
                self.pos += 1;
                let mut depth = 1usize;
                while self.pos < self.src.len() && depth > 0 {
                    match self.src[self.pos] {
                        b'\\' => self.pos += 1,
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        _ => {}
                    }
                    self.pos += 1;
                }
                Some(Token::Other)
            }
            _ if byte.is_ascii_digit() || byte == b'-' || byte == b'+' => {
                let start = self.pos;
                while self.pos < self.src.len() && self.src[self.pos].is_ascii_hexdigit() {
                    self.pos += 1;
                }
                Some(Token::Number(self.src[start..self.pos].to_vec()))
            }
            _ => {
                let start = self.pos;
                if self.src[self.pos] == b'/' {
                    self.pos += 1;
                }
                while self.pos < self.src.len() && !is_delimiter(self.src[self.pos]) {
                    self.pos += 1;
                }
                Some(Token::Word(self.src[start..self.pos].to_vec()))
            }
        }
    }

    /// Consume to the matching `>>` of a `<<` dictionary, honouring nested
    /// dictionaries and `(...)` strings well enough for a CMap header.
    fn skip_dictionary(&mut self) {
        let mut depth = 1usize;
        while self.pos < self.src.len() && depth > 0 {
            match self.src[self.pos] {
                b'<' => {
                    if self.src[self.pos + 1..].first() == Some(&b'<') {
                        depth += 1;
                        self.pos += 1;
                    }
                }
                b'>' => {
                    if self.src[self.pos + 1..].first() == Some(&b'>') {
                        depth -= 1;
                        self.pos += 1;
                    }
                }
                b'(' => {
                    self.pos += 1;
                    let mut string_depth = 1usize;
                    while self.pos < self.src.len() && string_depth > 0 {
                        match self.src[self.pos] {
                            b'\\' => self.pos += 1,
                            b'(' => string_depth += 1,
                            b')' => string_depth -= 1,
                            _ => {}
                        }
                        self.pos += 1;
                    }
                }
                _ => {}
            }
            self.pos += 1;
        }
    }

    /// Read up to `end` (exclusive), i.e. the body of a `begin…/end…` section.
    fn until(&mut self, end: &[u8]) -> Vec<Token> {
        let mut tokens = Vec::new();
        while let Some(token) = self.next() {
            if token.is_keyword(end) {
                return tokens;
            }
            tokens.push(token);
        }
        tokens
    }

    /// The byte lengths a `begincodespacerange` body declares.
    fn code_lengths(body: &[Token]) -> Vec<usize> {
        let mut lengths = Vec::new();
        let mut index = 0usize;
        while index < body.len() {
            if let Token::Hex(bytes) = &body[index] {
                lengths.push(bytes.len());
            }
            index += 1;
        }
        lengths
    }
}

fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b' ' | b'\t' | b'\r' | b'\n' | 0x0c | 0x0b | b'(' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Hex digits to bytes, left-padded with a zero nibble when the count is odd
/// (`<2A>` is one byte, `<2A3>` is `0x02 0xA3`, per the spec's zero extension).
fn hex_bytes(raw: &[u8]) -> Vec<u8> {
    let digits: Vec<u8> = raw
        .iter()
        .filter_map(|byte| nibble(*byte))
        .rev()
        .collect();
    let mut bytes = vec![0u8; (digits.len() + 1) / 2];
    for (index, digit) in digits.iter().enumerate() {
        let slot = bytes.len() - 1 - index / 2;
        bytes[slot] |= digit << (4 * (index % 2));
    }
    bytes
}

fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The header shape that cost the calibration book its Chinese: the
    /// `/CMapName` value has no leading slash, which is invalid PostScript.
    const BROKEN_HEADER: &str = r#"/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CMapName SimSun-cmap def
/CMapType 2 def
/CIDSystemInfo <<
/Registry (Adobe)
/Ordering (UCS)
/Supplement 0
>> def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
2 beginbfchar
<2D1E> <76EE>
<1585> <5F55>
endbfchar
endcmap
CMapName currentdict /CMap defineresource pop
end
end
"#;

    #[test]
    fn reads_a_cmap_with_a_malformed_header() {
        let map = UnicodeMap::parse(BROKEN_HEADER.as_bytes()).expect("parse");
        let decoded = map.decode(&[0x2D, 0x1E, 0x15, 0x85]);
        assert_eq!(decoded.text, "目录");
        assert_eq!(decoded.unmapped, 0);
        assert_eq!(map.mapped, 2);
    }

    #[test]
    fn bfrange_increment_and_array_forms() {
        let cmap = "\
/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CMapName /T-UCS2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
3 beginbfrange
<0041> <0043> <00F0>
<0100> <0101> [<2018> <2019>]
<0200> <0200> <4E00>
endbfrange
endcmap
end
end";
        let map = UnicodeMap::parse(cmap.as_bytes()).expect("parse");
        assert_eq!(map.decode(&[0x00, 0x41]).text, "ð");
        assert_eq!(map.decode(&[0x00, 0x42]).text, "ñ");
        assert_eq!(map.decode(&[0x00, 0x43]).text, "ò");
        assert_eq!(map.decode(&[0x01, 0x00]).text, "‘");
        assert_eq!(map.decode(&[0x01, 0x01]).text, "’");
        assert_eq!(map.decode(&[0x02, 0x00]).text, "一");
    }

    #[test]
    fn surrogate_pair_and_bom() {
        let cmap = "\
begincmap
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
2 beginbfchar
<0001> <0041>
<0002> <D835DC01>
<0003> <FEFF0042>
endbfchar
endcmap";
        let map = UnicodeMap::parse(cmap.as_bytes()).expect("parse");
        assert_eq!(map.decode(&[0x00, 0x01]).text, "A");
        assert_eq!(map.decode(&[0x00, 0x02]).text, "\u{1D401}");
        assert_eq!(map.decode(&[0x00, 0x03]).text, "B");
    }

    #[test]
    fn odd_hex_lengths_comments_and_unterminated_sections() {
        let cmap = "\
% a comment with <fake> hex in it
begincmap
1 begincodespacerange
<0> <FF>      % one-byte codes, written sloppily
endcodespacerange
1 beginbfchar
<2A> <4E2D>
<abc> <4e00>
endbfchar
endcmap";
        let map = UnicodeMap::parse(cmap.as_bytes()).expect("parse");
        assert_eq!(map.decode(&[0x2A]).text, "中");
        assert_eq!(map.decode(&[0x0A, 0xBC]).text, "一");
    }

    #[test]
    fn unmapped_codes_keep_the_run_aligned() {
        let cmap = "\
begincmap
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
1 beginbfchar
<0001> <4E00>
<0003> <4E8C>
endbfchar
endcmap";
        let map = UnicodeMap::parse(cmap.as_bytes()).expect("parse");
        // <0002> is not in the CMap: it must cost exactly two bytes, so the
        // codes after it stay on their boundaries.
        let decoded = map.decode(&[0x00, 0x01, 0x00, 0x02, 0x00, 0x03]);
        assert_eq!(decoded.text, "一二");
        assert_eq!(decoded.codes, 3);
        assert_eq!(decoded.unmapped, 1);
        assert_eq!(decoded.mapped(), 2);
    }

    #[test]
    fn two_byte_codes_are_never_matched_as_two_one_byte_codes() {
        let cmap = "\
begincmap
1 beginbfchar
<0024> <4E00>
<24> <41>
endbfchar
endcmap";
        let map = UnicodeMap::parse(cmap.as_bytes()).expect("parse");
        assert_eq!(map.decode(&[0x00, 0x24]).text, "一");
        assert_eq!(map.decode(&[0x24]).text, "A");
    }

    #[test]
    fn a_cmap_without_mappings_is_not_usable() {
        let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
                    1 begincodespacerange <0000> <FFFF> endcodespacerange endcmap end end";
        assert!(UnicodeMap::parse(cmap.as_bytes()).is_none());
    }

    #[test]
    fn huge_bfranges_are_capped() {
        let cmap = "\
begincmap
1 begincodespacerange
<00000000> <FFFFFF>
endcodespacerange
1 beginbfrange
<000000> <FFFFFF> <0041>
endbfrange
endcmap";
        let map = UnicodeMap::parse(cmap.as_bytes()).expect("parse");
        assert!(map.mapped <= MAX_RANGE_ENTRIES);
    }

    #[test]
    fn hex_pads_on_the_left() {
        assert_eq!(hex_bytes(b"2A"), vec![0x2A]);
        assert_eq!(hex_bytes(b"2A3"), vec![0x02, 0xA3]);
        assert_eq!(hex_bytes(b""), Vec::<u8>::new());
    }
}
