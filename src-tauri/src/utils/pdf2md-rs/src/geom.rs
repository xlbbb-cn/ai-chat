//! Small geometry primitives shared by the extractor and the layout analysis.
//!
//! Coordinate convention: after [`crate::extract::pdfoxide`] has run, every
//! rectangle is in **PDF points with a top-left origin** (`y` grows downwards),
//! which is what the layout heuristics expect. The PDF-native convention
//! (bottom-left origin, `y` up) only exists inside the content-stream
//! interpreter.

/// A 2×3 affine matrix `[a b c d e f]`, row-vector convention (PDF's `cm`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Default for Matrix {
    fn default() -> Self {
        Matrix::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub fn new(a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) -> Self {
        Matrix { a, b, c, d, e, f }
    }

    /// `self` then `other` (i.e. `self × other`, the order PDF composes in:
    /// text matrix first, then the current transformation matrix).
    pub fn mul(self, other: Matrix) -> Matrix {
        Matrix {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }

    /// Apply the affine map to a point.
    pub fn apply(self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    /// Average scale factor of the matrix, used to turn text-space sizes into
    /// device-space sizes.
    pub fn scale(self) -> f32 {
        let sx = (self.a * self.a + self.b * self.b).sqrt();
        let sy = (self.c * self.c + self.d * self.d).sqrt();
        ((sx * sy).sqrt()).max(0.0001)
    }

    /// Horizontal scale only (glyph widths advance along text-space `x`).
    pub fn scale_x(self) -> f32 {
        (self.a * self.a + self.b * self.b).sqrt().max(0.0001)
    }

    /// `true` when the matrix is a pure translation + uniform scale, i.e. text
    /// is not rotated or sheared.
    pub fn is_upright(self) -> bool {
        self.b.abs() < 0.01 && self.c.abs() < 0.01 && self.a > 0.0 && self.d > 0.0
    }
}

/// Axis-aligned rectangle, top-left origin, `y` growing downwards.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Rect { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }

    /// Build from a PDF-space box (`y` up) while flipping it into page space.
    pub fn from_pdf_box(x0: f32, y0: f32, x1: f32, y1: f32, page_height: f32) -> Self {
        Rect::new(x0, page_height - y1, x1, page_height - y0)
    }

    pub fn width(&self) -> f32 {
        (self.x1 - self.x0).max(0.0)
    }

    pub fn height(&self) -> f32 {
        (self.y1 - self.y0).max(0.0)
    }

    pub fn area(&self) -> f32 {
        self.width() * self.height()
    }

    pub fn center_x(&self) -> f32 {
        (self.x0 + self.x1) * 0.5
    }

    pub fn center_y(&self) -> f32 {
        (self.y0 + self.y1) * 0.5
    }

    pub fn union(&self, other: &Rect) -> Rect {
        Rect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    pub fn contains_point(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    /// Overlap area with another rectangle.
    pub fn intersection_area(&self, other: &Rect) -> f32 {
        let w = (self.x1.min(other.x1) - self.x0.max(other.x0)).max(0.0);
        let h = (self.y1.min(other.y1) - self.y0.max(other.y0)).max(0.0);
        w * h
    }

    /// `true` when the two boxes share at least `min_ratio` of the smaller
    /// one's area (overlap test that tolerates touching borders).
    pub fn overlaps(&self, other: &Rect, min_ratio: f32) -> bool {
        let smaller = self.area().min(other.area()).max(0.0001);
        self.intersection_area(other) / smaller >= min_ratio
    }

    /// Grow the rectangle by `margin` on every side.
    pub fn expand(&self, margin: f32) -> Rect {
        Rect { x0: self.x0 - margin, y0: self.y0 - margin, x1: self.x1 + margin, y1: self.y1 + margin }
    }

    /// Horizontal gap to another rectangle (negative when they overlap).
    pub fn horizontal_gap(&self, other: &Rect) -> f32 {
        (other.x0 - self.x1).max(self.x0 - other.x1)
    }

    /// `true` when the two rectangles are on the same visual line.
    pub fn same_row(&self, other: &Rect, tolerance: f32) -> bool {
        (self.center_y() - other.center_y()).abs() <= tolerance
    }
}

/// A horizontal band of a page, used for header/footer detection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub start: f32,
    pub end: f32,
}

impl Band {
    pub fn contains(&self, value: f32) -> bool {
        value >= self.start && value <= self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_composition_applies_text_then_ctm() {
        let ctm = Matrix::new(2.0, 0.0, 0.0, 2.0, 10.0, 20.0);
        let tm = Matrix::new(1.0, 0.0, 0.0, 1.0, 5.0, 0.0);
        let m = tm.mul(ctm);
        assert_eq!(m.apply(0.0, 0.0), (20.0, 20.0));
        assert_eq!(m.scale(), 2.0);
        assert!(m.is_upright());
    }

    #[test]
    fn pdf_box_is_flipped_to_top_left_origin() {
        let r = Rect::from_pdf_box(10.0, 700.0, 110.0, 720.0, 842.0);
        assert_eq!(r.y0, 122.0);
        assert_eq!(r.y1, 142.0);
        assert_eq!(r.width(), 100.0);
    }

    #[test]
    fn overlap_ratio_is_normalised_by_smaller_area() {
        let big = Rect::new(0.0, 0.0, 100.0, 100.0);
        let small = Rect::new(90.0, 90.0, 100.0, 100.0);
        assert!(small.overlaps(&big, 0.99));
        assert!(!big.overlaps(&small, 0.99));
    }

    #[test]
    fn gap_and_row_helpers() {
        let a = Rect::new(0.0, 10.0, 50.0, 20.0);
        let b = Rect::new(60.0, 11.0, 100.0, 21.0);
        assert_eq!(a.horizontal_gap(&b), 10.0);
        assert!(a.same_row(&b, 2.0));
    }
}
