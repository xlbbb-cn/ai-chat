//! PDFOxide — the built-in, pure-Rust content-stream interpreter.
//!
//! It walks a page's operators once and produces positioned glyphs, drawn
//! rules and placed images. All geometry is converted to **page space**
//! (points, top-left origin) on the way out, so nothing downstream has to know
//! about PDF's bottom-left origin, text matrices or the CTM stack.
//!
//! Supported: the graphics/text state operators that affect text placement
//! (`q Q cm BT ET Tf TL Tc Tw Tz Ts Td TD Tm T* Tj TJ ' "`), path construction
//! and painting (`re m l c v y h S s f F f* B B* b b* n w`), and XObjects (`Do`,
//! including form XObjects a few levels deep and inline images). Colour
//! operators are accepted and ignored — the renderer is monochrome text.

use lopdf::content::Operation;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::error::Result;
use crate::geom::{Matrix, Rect};

use super::fonts::{FontInfo, FontTable};
use super::images::{self, ImageData};

/// One positioned character (or ligature) in page space.
#[derive(Debug, Clone)]
pub struct Glyph {
    /// Decoded text — one character, or several when the font maps a code to a
    /// ligature (`ﬁ` → `fi`).
    pub text: String,
    pub rect: Rect,
    /// Effective font size in page-space points.
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub math: bool,
    /// Paint order, used as a tie-breaker when two glyphs share a position.
    pub seq: u32,
    /// Text drawn under a rotation/shear — excluded from line building.
    pub rotated: bool,
}

/// A drawn line or rectangle: table borders, separators, underline rules.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub rect: Rect,
    /// Short side of the rectangle, in points.
    pub thickness: f32,
    pub stroked: bool,
}

impl Rule {
    /// A rule that reads as a horizontal separator.
    pub fn is_horizontal(&self) -> bool {
        self.rect.width() >= self.rect.height() * 4.0 && self.rect.width() > 8.0
    }

    /// A rule that reads as a vertical separator.
    pub fn is_vertical(&self) -> bool {
        self.rect.height() >= self.rect.width() * 4.0 && self.rect.height() > 8.0
    }

    /// `true` when the rule is too short and thin to be a table border.
    pub fn is_furniture(&self) -> bool {
        self.rect.width() < 6.0 && self.rect.height() < 6.0
    }
}

/// A raster image placed on the page.
#[derive(Debug, Clone)]
pub struct ImagePlacement {
    pub rect: Rect,
    /// Resource name (`/Im3`) or `inline` for a `BI … EI` image.
    pub name: String,
    pub pixel_width: u32,
    pub pixel_height: u32,
    /// Decoded payload — `None` when extraction was disabled or unsupported.
    pub data: Option<ImageData>,
}

impl ImagePlacement {
    /// `true` when the placement is large enough to be a real figure.
    pub fn is_figure_sized(&self, min_points: f32) -> bool {
        self.rect.width() >= min_points && self.rect.height() >= min_points
    }
}

/// Everything one page's content stream yielded.
#[derive(Debug, Default, Clone)]
pub struct PageContent {
    pub glyphs: Vec<Glyph>,
    pub rules: Vec<Rule>,
    pub images: Vec<ImagePlacement>,
}

/// Graphics + text state that `q`/`Q` save and restore.
#[derive(Clone)]
struct GState {
    ctm: Matrix,
    font: Option<Vec<u8>>,
    font_size: f32,
    char_spacing: f32,
    word_spacing: f32,
    h_scale: f32,
    leading: f32,
    rise: f32,
    line_width: f32,
    tm: Matrix,
    tlm: Matrix,
}

impl Default for GState {
    fn default() -> Self {
        GState {
            ctm: Matrix::IDENTITY,
            font: None,
            font_size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            h_scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            line_width: 1.0,
            tm: Matrix::IDENTITY,
            tlm: Matrix::IDENTITY,
        }
    }
}

/// Path under construction (user space).
#[derive(Clone, Default)]
struct PathState {
    segments: Vec<((f32, f32), (f32, f32))>,
    rects: Vec<(f32, f32, f32, f32)>,
    current: Option<(f32, f32)>,
    start: Option<(f32, f32)>,
}

/// Parse and interpret one page.
pub(crate) fn interpret_page(
    doc: &Document,
    page_id: ObjectId,
    page_height: f32,
    fonts: &FontTable<'_>,
    collect_images: bool,
) -> Result<PageContent> {
    let content = doc.get_and_decode_page_content(page_id)?;
    let resources = doc.get_page_resources(page_id).ok().and_then(|(dict, _)| dict);
    let mut interpreter = Interpreter {
        doc,
        page_height,
        fonts,
        collect_images,
        out: PageContent::default(),
        seq: 0,
        depth: 0,
        path: PathState::default(),
    };
    interpreter.run(&content.operations, resources, GState::default());
    Ok(interpreter.out)
}

struct Interpreter<'a> {
    doc: &'a Document,
    page_height: f32,
    fonts: &'a FontTable<'a>,
    collect_images: bool,
    out: PageContent,
    seq: u32,
    depth: u8,
    path: PathState,
}

impl<'a> Interpreter<'a> {
    fn run(&mut self, operations: &[Operation], resources: Option<&'a Dictionary>, seed: GState) {
        let mut state = seed;
        let mut stack: Vec<GState> = Vec::new();
        for op in operations {
            match op.operator.as_str() {
                "q" => stack.push(state.clone()),
                "Q" => {
                    if let Some(previous) = stack.pop() {
                        state = previous;
                    }
                }
                "cm" => {
                    if let Some(m) = matrix_operand(&op.operands) {
                        state.ctm = m * state.ctm;
                    }
                }
                "BT" => {
                    state.tm = Matrix::IDENTITY;
                    state.tlm = Matrix::IDENTITY;
                }
                "ET" => {}
                "Tf" => {
                    if let Some(Object::Name(name)) = op.operands.first() {
                        state.font = Some(name.clone());
                    }
                    if let Some(size) = float_at(&op.operands, 1) {
                        state.font_size = size;
                    }
                }
                "TL" => state.leading = float_at(&op.operands, 0).unwrap_or(0.0),
                "Tc" => state.char_spacing = float_at(&op.operands, 0).unwrap_or(0.0),
                "Tw" => state.word_spacing = float_at(&op.operands, 0).unwrap_or(0.0),
                "Tz" => state.h_scale = float_at(&op.operands, 0).unwrap_or(100.0) / 100.0,
                "Ts" => state.rise = float_at(&op.operands, 0).unwrap_or(0.0),
                "Td" => {
                    let tx = float_at(&op.operands, 0).unwrap_or(0.0);
                    let ty = float_at(&op.operands, 1).unwrap_or(0.0);
                    state.tlm = translate(tx, ty) * state.tlm;
                    state.tm = state.tlm;
                }
                "TD" => {
                    let tx = float_at(&op.operands, 0).unwrap_or(0.0);
                    let ty = float_at(&op.operands, 1).unwrap_or(0.0);
                    state.leading = -ty;
                    state.tlm = translate(tx, ty) * state.tlm;
                    state.tm = state.tlm;
                }
                "Tm" => {
                    if let Some(m) = matrix_operand(&op.operands) {
                        state.tlm = m;
                        state.tm = m;
                    }
                }
                "T*" => {
                    state.tlm = translate(0.0, -state.leading) * state.tlm;
                    state.tm = state.tlm;
                }
                "Tj" => {
                    if let Some(Object::String(bytes, _)) = op.operands.first() {
                        self.show_text(&mut state, bytes);
                    }
                }
                "TJ" => {
                    if let Some(Object::Array(items)) = op.operands.first() {
                        self.show_array(&mut state, items);
                    }
                }
                "'" => {
                    state.tlm = translate(0.0, -state.leading) * state.tlm;
                    state.tm = state.tlm;
                    if let Some(Object::String(bytes, _)) = op.operands.first() {
                        self.show_text(&mut state, bytes);
                    }
                }
                "\"" => {
                    if let Some(value) = float_at(&op.operands, 0) {
                        state.word_spacing = value;
                    }
                    if let Some(value) = float_at(&op.operands, 1) {
                        state.char_spacing = value;
                    }
                    state.tlm = translate(0.0, -state.leading) * state.tlm;
                    state.tm = state.tlm;
                    if let Some(Object::String(bytes, _)) = op.operands.get(2) {
                        self.show_text(&mut state, bytes);
                    }
                }
                "w" => state.line_width = float_at(&op.operands, 0).unwrap_or(1.0),
                "re" => {
                    let x = float_at(&op.operands, 0).unwrap_or(0.0);
                    let y = float_at(&op.operands, 1).unwrap_or(0.0);
                    let w = float_at(&op.operands, 2).unwrap_or(0.0);
                    let h = float_at(&op.operands, 3).unwrap_or(0.0);
                    self.path.rects.push((x, y, w, h));
                    self.path.current = Some((x, y));
                    self.path.start = Some((x, y));
                }
                "m" => {
                    let point = (
                        float_at(&op.operands, 0).unwrap_or(0.0),
                        float_at(&op.operands, 1).unwrap_or(0.0),
                    );
                    self.path.current = Some(point);
                    self.path.start = Some(point);
                }
                "l" => {
                    let point = (
                        float_at(&op.operands, 0).unwrap_or(0.0),
                        float_at(&op.operands, 1).unwrap_or(0.0),
                    );
                    if let Some(from) = self.path.current {
                        self.path.segments.push((from, point));
                    }
                    self.path.current = Some(point);
                }
                // Curves contribute their end point; that is all a border needs.
                "c" | "v" | "y" => {
                    let index = op.operands.len().saturating_sub(2);
                    let point = (
                        float_at(&op.operands, index).unwrap_or(0.0),
                        float_at(&op.operands, index + 1).unwrap_or(0.0),
                    );
                    if op.operator == "c" {
                        if let Some(from) = self.path.current {
                            self.path.segments.push((from, point));
                        }
                    }
                    self.path.current = Some(point);
                }
                "h" => {
                    if let (Some(start), Some(current)) = (self.path.start, self.path.current) {
                        self.path.segments.push((current, start));
                    }
                    self.path.current = self.path.start;
                }
                "S" | "s" | "B" | "B*" | "b" | "b*" => {
                    self.flush_path(&state, true);
                    if op.operator == "s" || op.operator == "b" {
                        self.path.current = self.path.start;
                    }
                }
                "f" | "F" | "f*" => self.flush_path(&state, false),
                "n" | "W" | "W*" => {
                    self.path.segments.clear();
                    self.path.rects.clear();
                }
                "Do" => {
                    if let Some(Object::Name(name)) = op.operands.first() {
                        self.draw_xobject(&state, name, resources);
                    }
                }
                "BI" => {
                    if let Some(Object::Stream(stream)) = op.operands.first() {
                        self.place_image(&state, stream, "inline".to_string());
                    }
                }
                _ => {}
            }
        }
        self.flush_path(&state, false);
    }

    /// Emit a string operand as positioned glyphs.
    fn show_text(&mut self, state: &mut GState, bytes: &[u8]) {
        let Some(font_name) = state.font.clone() else {
            return;
        };
        let fallback = FontInfo::default();
        let info = self.fonts.info(&font_name).unwrap_or(&fallback).clone();
        let text = self.fonts.decode(&font_name, bytes);
        // The font's own widths are indexed by *code*, so they can only be used
        // when the decoding is one byte per character. A string that decodes to
        // a different number of characters (a CMap, a UTF-8 sniff, a dropped
        // byte) falls back to the estimate for the whole run — mixing the two
        // would place the run wrong from the first mismatch on.
        let codes: Option<&[u8]> = if text.chars().count() == bytes.len() { Some(bytes) } else { None };
        for (index, ch) in text.chars().enumerate() {
            let width = codes.and_then(|codes| info.advance_code(codes[index]));
            self.emit_glyph(state, &info, ch, width);
        }
    }

    /// `TJ` array: strings advance the text matrix, numbers nudge it.
    fn show_array(&mut self, state: &mut GState, items: &[Object]) {
        for item in items {
            match item {
                Object::String(bytes, _) => self.show_text(state, bytes),
                Object::Integer(value) => self.kern(state, *value as f32),
                Object::Real(value) => self.kern(state, *value),
                _ => {}
            }
        }
    }

    fn kern(&mut self, state: &mut GState, amount: f32) {
        let shift = -amount / 1000.0 * state.font_size * state.h_scale;
        state.tm = translate(shift, 0.0) * state.tm;
    }

    /// Place one character and advance the text matrix past it.
    ///
    /// `width_em` is the font's own advance for the character's code when the
    /// PDF provides it, and `None` for the estimate.
    fn emit_glyph(&mut self, state: &mut GState, info: &FontInfo, ch: char, width_em: Option<f32>) {
        let width_em = width_em.unwrap_or_else(|| info.advance(ch));
        let mut advance = width_em * state.font_size + state.char_spacing;
        if ch == ' ' {
            advance += state.word_spacing;
        }
        advance *= state.h_scale;

        let trm = state.tm * state.ctm;
        let start = trm.apply(0.0, state.rise);
        let end = trm.apply(advance, state.rise);

        if !ch.is_control() && !ch.is_whitespace() {
            let size = state.font_size.max(0.1) * trm.scale();
            let baseline = self.page_height - start.1;
            let top = baseline - info.ascent * size;
            let rect = Rect::new(start.0.min(end.0), top, start.0.max(end.0), top + size);
            self.seq += 1;
            self.out.glyphs.push(Glyph {
                text: ch.to_string(),
                rect,
                size,
                bold: info.bold,
                italic: info.italic,
                mono: info.mono,
                math: info.math,
                seq: self.seq,
                rotated: !trm.is_upright(),
            });
        }

        state.tm = translate(advance, 0.0) * state.tm;
    }

    /// Turn the current path into rules (thin) and rectangles (table borders).
    fn flush_path(&mut self, state: &GState, stroked: bool) {
        let ctm = state.ctm;
        let thickness = (state.line_width * ctm.scale()).max(0.2);

        for (x, y, w, h) in std::mem::take(&mut self.path.rects) {
            let corners = [
                ctm.apply(x, y),
                ctm.apply(x + w, y),
                ctm.apply(x + w, y + h),
                ctm.apply(x, y + h),
            ];
            let rect = rect_from_points(&corners, self.page_height);
            if rect.area() <= 0.5 {
                continue;
            }
            self.out.rules.push(Rule { rect, thickness, stroked });
        }

        for (from, to) in std::mem::take(&mut self.path.segments) {
            let a = ctm.apply(from.0, from.1);
            let b = ctm.apply(to.0, to.1);
            let mut rect = rect_from_points(&[a, b], self.page_height);
            if rect.width() < thickness {
                let center = rect.center_x();
                rect.x0 = center - thickness * 0.5;
                rect.x1 = center + thickness * 0.5;
            }
            if rect.height() < thickness {
                let center = rect.center_y();
                rect.y0 = center - thickness * 0.5;
                rect.y1 = center + thickness * 0.5;
            }
            self.out.rules.push(Rule { rect, thickness, stroked });
        }

        self.path.current = None;
        self.path.start = None;
    }

    /// `Do` — images are recorded (and decoded), forms are followed.
    fn draw_xobject(&mut self, state: &GState, name: &[u8], resources: Option<&'a Dictionary>) {
        let Some(resources) = resources else {
            return;
        };
        let Some(stream) = lookup_xobject(self.doc, resources, name) else {
            return;
        };
        let subtype = stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(|o| o.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).to_string())
            .unwrap_or_default();
        match subtype.as_str() {
            "Image" => self.place_image(state, stream, String::from_utf8_lossy(name).to_string()),
            "Form" => self.run_form(state, stream),
            _ => {}
        }
    }

    fn place_image(&mut self, state: &GState, stream: &Stream, name: String) {
        let corners = [
            state.ctm.apply(0.0, 0.0),
            state.ctm.apply(1.0, 0.0),
            state.ctm.apply(1.0, 1.0),
            state.ctm.apply(0.0, 1.0),
        ];
        let rect = rect_from_points(&corners, self.page_height);
        if rect.width() < 2.0 || rect.height() < 2.0 {
            return;
        }
        let pixel_width = stream
            .dict
            .get(b"Width")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0)
            .max(0) as u32;
        let pixel_height = stream
            .dict
            .get(b"Height")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0)
            .max(0) as u32;
        let data = if self.collect_images {
            images::extract(stream).ok()
        } else {
            None
        };
        self.out.images.push(ImagePlacement { rect, name, pixel_width, pixel_height, data });
    }

    /// Follow a form XObject, applying its `/Matrix`, a few levels deep.
    fn run_form(&mut self, state: &GState, stream: &'a Stream) {
        if self.depth >= 3 {
            return;
        }
        let Ok(bytes) = stream.decompressed_content() else {
            return;
        };
        let Ok(content) = lopdf::content::Content::decode(&bytes) else {
            return;
        };
        let mut inner = state.clone();
        if let Ok(matrix) = stream.dict.get(b"Matrix") {
            if let Ok(items) = matrix.as_array() {
                let values: Vec<f32> = items.iter().filter_map(|o| o.as_float().ok()).collect();
                if values.len() == 6 {
                    inner.ctm = Matrix::new(
                        values[0], values[1], values[2], values[3], values[4], values[5],
                    ) * inner.ctm;
                }
            }
        }
        let resources = stream
            .dict
            .get_deref(b"Resources", self.doc)
            .ok()
            .and_then(|o| o.as_dict().ok());
        self.depth += 1;
        let saved_path = std::mem::take(&mut self.path);
        self.run(&content.operations, resources, inner);
        self.path = saved_path;
        self.depth -= 1;
    }
}

fn translate(tx: f32, ty: f32) -> Matrix {
    Matrix::new(1.0, 0.0, 0.0, 1.0, tx, ty)
}

fn rect_from_points(points: &[(f32, f32)], page_height: f32) -> Rect {
    let mut x0 = f32::MAX;
    let mut x1 = f32::MIN;
    let mut y0 = f32::MAX;
    let mut y1 = f32::MIN;
    for (x, y) in points {
        let y = page_height - y;
        x0 = x0.min(*x);
        x1 = x1.max(*x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    Rect::new(x0, y0, x1, y1)
}

fn float_at(operands: &[Object], index: usize) -> Option<f32> {
    operands.get(index).and_then(|o| o.as_float().ok())
}

fn matrix_operand(operands: &[Object]) -> Option<Matrix> {
    let values: Vec<f32> = operands.iter().take(6).filter_map(|o| o.as_float().ok()).collect();
    if values.len() == 6 {
        Some(Matrix::new(values[0], values[1], values[2], values[3], values[4], values[5]))
    } else {
        None
    }
}

fn lookup_xobject<'a>(
    doc: &'a Document,
    resources: &'a Dictionary,
    name: &[u8],
) -> Option<&'a Stream> {
    let xobjects = resources.get_deref(b"XObject", doc).ok()?.as_dict().ok()?;
    let entry = xobjects.get(name).ok()?;
    let (_, object) = doc.dereference(entry).ok()?;
    object.as_stream().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_from_points_flips_y() {
        let r = rect_from_points(&[(0.0, 0.0), (10.0, 10.0)], 100.0);
        assert_eq!(r, Rect::new(0.0, 90.0, 10.0, 100.0));
    }

    #[test]
    fn rule_orientation_needs_a_four_to_one_ratio() {
        let horizontal =
            Rule { rect: Rect::new(0.0, 10.0, 100.0, 10.5), thickness: 0.5, stroked: true };
        assert!(horizontal.is_horizontal());
        let vertical =
            Rule { rect: Rect::new(10.0, 0.0, 10.5, 100.0), thickness: 0.5, stroked: true };
        assert!(vertical.is_vertical());
        let square = Rule { rect: Rect::new(0.0, 0.0, 10.0, 10.0), thickness: 1.0, stroked: true };
        assert!(!square.is_horizontal() && !square.is_vertical());
        let dot = Rule { rect: Rect::new(0.0, 0.0, 2.0, 2.0), thickness: 1.0, stroked: false };
        assert!(dot.is_furniture());
    }

    #[test]
    fn text_matrix_advance_moves_the_pen() {
        let mut state = GState { font_size: 10.0, ..GState::default() };
        let info = FontInfo::default();
        let trm_before = state.tm * state.ctm;
        let advance = info.advance('A') * 10.0;
        state.tm = translate(advance, 0.0) * state.tm;
        let trm_after = state.tm * state.ctm;
        assert!((trm_after.e - trm_before.e - advance).abs() < 0.001);
        assert!((trm_after.f - trm_before.f).abs() < 0.001);
    }
}
