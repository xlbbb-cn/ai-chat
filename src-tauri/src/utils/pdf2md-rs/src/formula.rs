//! Formula support.
//!
//! A PDF has no notion of a formula. What it has is glyphs, fonts and positions,
//! so there are exactly three honest possibilities, and this module covers all
//! of them without pretending otherwise:
//!
//! 1. **Text formulas** — the equation is a text object with ordinary glyphs. The
//!    geometry is enough to *find* it (centred, short, math font, math symbols),
//!    and it can be transcribed approximately: unicode → LaTeX for the commands
//!    we know, `\text{…}` for everything else. [`HeuristicFormula`] does this.
//! 2. **Image formulas** — the equation is an embedded image (LaTeX-rendered
//!    papers very often do this). Only a real recognizer can read it; plug one in
//!    through [`FormulaRecognizer`] and [`crate::options::FormulaOptions`].
//! 3. **Vector formulas** — the equation is drawn as paths. Neither of the above
//!    applies; the region is kept as text if any exists, and skipped otherwise.
//!
//! The pipeline never invents a formula: when confidence is low the paragraph is
//! emitted as ordinary text instead.

use crate::extract::pdfoxide::ImageFormat;
use crate::error::Result;
use crate::geom::Rect;
use crate::ir::Block;
use crate::layout::{PageLayout, Paragraph};

/// An image-based formula recognizer (pix2tex, Mathpix, a local model…).
///
/// Implement it and pass it in [`crate::options::FormulaOptions::recognizer`];
/// the pipeline calls it for every image that sits alone in a centred box.
///
/// ```ignore
/// struct MyOcr;
/// impl pdf2md_rs::formula::FormulaRecognizer for MyOcr {
///     fn name(&self) -> &str { "my-ocr" }
///     fn recognize(&self, image: &[u8], format: pdf2md_rs::extract::ImageFormat)
///         -> pdf2md_rs::Result<String>
///     {
///         // … call the model, return LaTeX …
///         # unimplemented!()
///     }
/// }
/// ```
///
/// The signature is deliberately narrow so a `anyhow`-based plugin wraps in one
/// line at the call site.
pub trait FormulaRecognizer: Send + Sync {
    /// Short identifier, used in logs and in [`crate::options::FormulaOptions`]'s
    /// `Debug` output.
    fn name(&self) -> &str;

    /// Return LaTeX source (without `$` delimiters) for the image.
    fn recognize(&self, image: &[u8], format: ImageFormat) -> Result<String>;
}

/// A formula found in the text layer.
#[derive(Debug, Clone, PartialEq)]
pub struct FormulaCandidate {
    /// Best-effort LaTeX for the formula.
    pub latex: String,
    pub rect: Rect,
    /// 0..=1 — how strongly the geometry says "this is an equation".
    pub confidence: f32,
}

/// Detect and transcribe text-layer formulas.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicFormula;

impl HeuristicFormula {
    /// Return the formula when `paragraph` looks like a displayed equation.
    ///
    /// The threshold is intentionally high: a false positive deletes readable
    /// text, a false negative only costs a nicer rendering.
    pub fn detect(&self, layout: &PageLayout, paragraph: &Paragraph) -> Option<FormulaCandidate> {
        if paragraph.lines.len() > 3 {
            return None;
        }
        let column = layout.columns.columns.get(paragraph.column)?;
        let width = paragraph.rect.width();
        if width < column.width() * 0.06 || width > column.width() * 0.88 {
            return None;
        }
        // Centred in its column?
        let centre_offset = (paragraph.rect.center_x() - column.center()).abs();
        let centred = centre_offset <= column.width() * 0.08;

        let text = layout.paragraph_text(paragraph);
        let symbol_ratio = math_symbol_ratio(&text);
        let score = 0.45 * f32::from(centred)
            + 0.4 * paragraph.math_ratio
            + 0.5 * symbol_ratio
            + if ends_like_sentence(&text) { -0.35 } else { 0.1 };
        if score < 0.55 {
            return None;
        }
        let latex = to_latex(&text);
        if latex.trim().is_empty() {
            return None;
        }
        Some(FormulaCandidate { latex, rect: paragraph.rect, confidence: score.min(1.0) })
    }

    /// Build the IR block for a candidate.
    pub fn block(&self, candidate: &FormulaCandidate) -> Block {
        Block::Formula { latex: candidate.latex.clone(), display: true }
    }
}

/// Fraction of characters that are math symbols.
pub fn math_symbol_ratio(text: &str) -> f32 {
    let mut symbols = 0usize;
    let mut total = 0usize;
    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        total += 1;
        if is_math_symbol(ch) {
            symbols += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        symbols as f32 / total as f32
    }
}

/// Characters that almost never appear in prose.
pub fn is_math_symbol(ch: char) -> bool {
    matches!(ch,
        '=' | '+' | '×' | '÷' | '±' | '∓' | '∑' | '∏' | '∫' | '√' | '∞' | '≈' | '≠' | '≤' | '≥'
        | '≪' | '≫' | '≡' | '∝' | '∈' | '∉' | '⊂' | '⊃' | '∪' | '∩' | '∀' | '∃' | '∂' | '∇'
        | '→' | '←' | '↔' | '⇒' | '⇔' | '⋅' | '∘' | '⊗' | '⊕' | '⊥' | '∥' | 'α'..='ω'
        | 'Α'..='Ω' | '^' | '_' | '·'
    )
}

fn ends_like_sentence(text: &str) -> bool {
    let trimmed = text.trim_end();
    trimmed.ends_with('.') || trimmed.ends_with('。') || trimmed.ends_with(';')
}

/// Unicode → LaTeX for the symbols we can name, `\text{…}` for the rest.
///
/// This is a transcription, not a mathematical reconstruction: superscripts and
/// radicals come out as `^{…}` / `\sqrt{…}` only when the PDF used real
/// super/subscript glyph runs, and fractions are not rebuilt at all.
pub fn to_latex(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut pending_text = String::new();
    let mut flush = |pending: &mut String, out: &mut String| {
        if pending.is_empty() {
            return;
        }
        let escaped = escape_latex(pending);
        out.push_str(&escaped);
        pending.clear();
    };

    for ch in text.chars() {
        match latex_for(ch) {
            Some(command) => {
                flush(&mut pending_text, &mut out);
                out.push_str(command);
            }
            None => pending_text.push(ch),
        }
    }
    flush(&mut pending_text, &mut out);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn latex_for(ch: char) -> Option<&'static str> {
    Some(match ch {
        'α' => "\\alpha",
        'β' => "\\beta",
        'γ' => "\\gamma",
        'δ' => "\\delta",
        'ε' | 'ϵ' => "\\epsilon",
        'ζ' => "\\zeta",
        'η' => "\\eta",
        'θ' | 'ϑ' => "\\theta",
        'ι' => "\\iota",
        'κ' => "\\kappa",
        'λ' => "\\lambda",
        'μ' | 'µ' => "\\mu",
        'ν' => "\\nu",
        'ξ' => "\\xi",
        'π' | 'ϖ' => "\\pi",
        'ρ' | 'ϱ' => "\\rho",
        'σ' | 'ς' => "\\sigma",
        'τ' => "\\tau",
        'υ' => "\\upsilon",
        'φ' | 'ϕ' => "\\phi",
        'χ' => "\\chi",
        'ψ' => "\\psi",
        'ω' => "\\omega",
        'Γ' => "\\Gamma",
        'Δ' => "\\Delta",
        'Θ' => "\\Theta",
        'Λ' => "\\Lambda",
        'Ξ' => "\\Xi",
        'Π' => "\\Pi",
        'Σ' => "\\Sigma",
        'Υ' => "\\Upsilon",
        'Φ' => "\\Phi",
        'Ψ' => "\\Psi",
        'Ω' => "\\Omega",
        '×' => "\\times",
        '÷' => "\\div",
        '±' => "\\pm",
        '∓' => "\\mp",
        '⋅' | '·' => "\\cdot",
        '∘' => "\\circ",
        '∗' => "\\ast",
        '≤' => "\\leq",
        '≥' => "\\geq",
        '≠' => "\\neq",
        '≈' => "\\approx",
        '≡' => "\\equiv",
        '∝' => "\\propto",
        '≪' => "\\ll",
        '≫' => "\\gg",
        '∞' => "\\infty",
        '∑' => "\\sum",
        '∏' => "\\prod",
        '∫' => "\\int",
        '∮' => "\\oint",
        '√' => "\\sqrt",
        '∂' => "\\partial",
        '∇' => "\\nabla",
        '∈' => "\\in",
        '∉' => "\\notin",
        '⊂' => "\\subset",
        '⊃' => "\\supset",
        '∪' => "\\cup",
        '∩' => "\\cap",
        '∀' => "\\forall",
        '∃' => "\\exists",
        '→' => "\\to",
        '←' => "\\leftarrow",
        '↔' => "\\leftrightarrow",
        '⇒' => "\\Rightarrow",
        '⇔' => "\\Leftrightarrow",
        '⊗' => "\\otimes",
        '⊕' => "\\oplus",
        '⊥' => "\\perp",
        '∥' => "\\parallel",
        '…' => "\\ldots",
        '⟨' => "\\langle",
        '⟩' => "\\rangle",
        '⌈' => "\\lceil",
        '⌉' => "\\rceil",
        '⌊' => "\\lfloor",
        '⌋' => "\\rfloor",
        _ => return None,
    })
}

/// Escape the characters LaTeX would otherwise interpret.
pub fn escape_latex(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\backslash "),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '%' => out.push_str("\\%"),
            '&' => out.push_str("\\&"),
            '#' => out.push_str("\\#"),
            '$' => out.push_str("\\$"),
            '_' => out.push_str("\\_"),
            '~' => out.push_str("\\sim "),
            '^' => out.push_str("\\hat{} "),
            _ => out.push(ch),
        }
    }
    out
}

/// Wrap the math-looking runs of a paragraph in `$…$`.
///
/// Conservative on purpose: a run is only wrapped when it contains an
/// unmistakable math symbol (`=`, `≤`, `∑`, a Greek letter…) or has an index
/// pattern (`x[n]`, `h_i`). Ordinary prose, code and numbers stay untouched, and
/// text that already contains `$` is left alone.
pub fn wrap_inline_math(text: &str) -> String {
    if text.contains('$') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut run: Vec<&str> = Vec::new();
    let mut separator = String::new();

    for token in split_keep_spaces(text) {
        if token.trim().is_empty() {
            separator.push_str(token);
            continue;
        }
        if looks_like_math(token) {
            if run.is_empty() {
                out.push_str(&separator);
                separator.clear();
            } else {
                out.push_str(&separator);
                separator.clear();
            }
            run.push(token);
        } else {
            if !run.is_empty() {
                out.push('$');
                out.push_str(&run.join(" "));
                out.push('$');
                run.clear();
            }
            out.push_str(&separator);
            separator.clear();
            out.push_str(token);
        }
    }
    if !run.is_empty() {
        out.push('$');
        out.push_str(&run.join(" "));
        out.push('$');
    }
    out.push_str(&separator);
    out
}

fn split_keep_spaces(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut in_space = false;
    for (index, ch) in text.char_indices() {
        let space = ch.is_whitespace();
        if index == 0 {
            in_space = space;
            continue;
        }
        if space != in_space {
            tokens.push(&text[start..index]);
            start = index;
            in_space = space;
        }
    }
    tokens.push(&text[start..]);
    tokens
}

fn looks_like_math(token: &str) -> bool {
    let trimmed = token.trim_matches(|c: char| c.is_ascii_punctuation() && c != '[' && c != ']');
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.chars().any(is_math_symbol) && trimmed.chars().any(|c| c.is_alphanumeric()) {
        return true;
    }
    // `x[n]`, `h_i`, `f(x)^2`
    let mut chars = trimmed.chars().peekable();
    let mut saw_variable = false;
    while let Some(ch) = chars.next() {
        if ch.is_alphabetic() {
            saw_variable = true;
            continue;
        }
        if saw_variable && matches!(ch, '[' | '_') {
            // Require a closing bracket for `[` and at least one char after `_`.
            if ch == '[' && trimmed.contains(']') {
                return true;
            }
            if ch == '_' && chars.peek().is_some_and(|c| c.is_alphanumeric() || *c == '{') {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greek_and_operators_become_commands() {
        assert_eq!(to_latex("α ≤ β"), "\\alpha \\leq \\beta");
        assert_eq!(to_latex("a×b"), "a\\times b");
    }

    #[test]
    fn special_characters_are_escaped() {
        assert_eq!(escape_latex("50% of a_b"), "50\\% of a\\_b");
    }

    #[test]
    fn inline_math_wraps_only_symbol_runs() {
        assert_eq!(wrap_inline_math("we have x[n] = h[n] here"), "we have $x[n] = h[n]$ here");
        assert_eq!(wrap_inline_math("plain prose stays plain"), "plain prose stays plain");
        // Already-marked text is left alone.
        assert_eq!(wrap_inline_math("keep $x$ as is"), "keep $x$ as is");
    }

    #[test]
    fn inline_math_needs_a_variable() {
        assert!(!looks_like_math("="));
        assert!(looks_like_math("y[n]"));
        assert!(looks_like_math("h_i"));
        assert!(!looks_like_math("hello"));
    }

    #[test]
    fn sentence_like_centre_text_is_not_a_formula() {
        let paragraph = Paragraph {
            lines: vec![0],
            rect: Rect::new(200.0, 100.0, 300.0, 112.0),
            size: 10.0,
            bold: false,
            italic: false,
            mono: false,
            math_ratio: 0.0,
            column: 0,
            footnote: false,
        };
        let layout = PageLayout::default();
        assert!(HeuristicFormula.detect(&layout, &paragraph).is_none());
    }
}
