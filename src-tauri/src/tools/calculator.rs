//! The `calculator` tool: a small, self-contained expression evaluator.
//!
//! No external crate and no `eval`: expressions are tokenized, parsed with a
//! recursive-descent parser into an AST, then evaluated over `Number` (an
//! `i64`/`f64` sum) so integer-only operators (factorial, bitwise) stay exact.
//!
//! Grammar (lowest to highest precedence):
//!   or        := xor (('or' | '|') xor)*
//!   xor       := and (('xor' | '^^') and)*
//!   and       := shift (('and' | '&') shift)*
//!   shift     := sum (('<<' | '>>') sum)*
//!   sum       := product (('+' | '-') product)*
//!   product   := unary (('*' | '/' | '%' | implicit) unary)*
//!   unary     := ('+' | '-' | '~') unary | power
//!   power     := postfix (('^' | '**') unary)?        // right associative
//!   postfix   := primary ('!')*
//!   primary   := number | constant | call | '(' or ')'

use std::cell::Cell;

use serde_json::{json, Value};

const MAX_INPUT_CHARS: usize = 4096;
const MAX_TOKENS: usize = 1024;
const MAX_DEPTH: usize = 128;

// ── Values ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum Number {
    Int(i64),
    Float(f64),
}

fn to_f64(n: Number) -> f64 {
    match n {
        Number::Int(i) => i as f64,
        Number::Float(f) => f,
    }
}

/// Integer-only operators require an actual integer (an integral float such
/// as `5.0` is accepted, a fractional one is a domain error).
fn as_int(n: Number) -> Result<i64, String> {
    match n {
        Number::Int(i) => Ok(i),
        Number::Float(f)
            if f.is_finite() && f.fract() == 0.0 && f >= i64::MIN as f64 && f < i64::MAX as f64 =>
        {
            Ok(f as i64)
        }
        _ => Err("this operator requires an integer operand".to_string()),
    }
}

fn finite(n: Number) -> Result<Number, String> {
    match n {
        Number::Float(f) if !f.is_finite() => Err(
            "result is not a finite real number (division by zero, overflow, or a non-real exponent)"
                .to_string(),
        ),
        other => Ok(other),
    }
}

// ── Tokens ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    Number(Number),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    DoubleStar,
    Bang,
    Amp,
    Pipe,
    Tilde,
    DoubleCaret,
    Shl,
    Shr,
    LParen,
    RParen,
    Comma,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    pos: usize, // zero-based character index, for error messages
}

fn is_operator_ident(s: &str) -> bool {
    s.eq_ignore_ascii_case("or") || s.eq_ignore_ascii_case("xor") || s.eq_ignore_ascii_case("and")
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < len {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let pos = i;

        // Numbers: radix literals first (`0x`, `0b`, `0o`), then decimal /
        // scientific. A leading '.' only starts a number when a digit follows.
        if c.is_ascii_digit() || (c == '.' && i + 1 < len && chars[i + 1].is_ascii_digit()) {
            if c == '0' && i + 1 < len && matches!(chars[i + 1], 'x' | 'X' | 'b' | 'B' | 'o' | 'O')
            {
                let radix = match chars[i + 1] {
                    'x' | 'X' => 16,
                    'b' | 'B' => 2,
                    _ => 8,
                };
                i += 2;
                let start_digits = i;
                while i < len && (chars[i].is_digit(radix) || chars[i] == '_') {
                    i += 1;
                }
                if i == start_digits {
                    return Err(format!(
                        "Radix literal has no digits at position {}",
                        pos + 1
                    ));
                }
                // `0b102` must be an error, not `0b10 * 2`: a digit the radix
                // cannot represent directly after the literal is a typo.
                if i < len && chars[i].is_ascii_alphanumeric() {
                    return Err(format!(
                        "Invalid digit '{}' in radix literal at position {}",
                        chars[i],
                        i + 1
                    ));
                }
                let raw: String = chars[start_digits..i]
                    .iter()
                    .filter(|ch| **ch != '_')
                    .collect();
                let value = i64::from_str_radix(&raw, radix).map_err(|_| {
                    format!("Invalid radix literal '{raw}' at position {}", pos + 1)
                })?;
                out.push(Token {
                    kind: TokenKind::Number(Number::Int(value)),
                    pos,
                });
                continue;
            }

            let mut seen_dot = false;
            let mut seen_exp = false;
            while i < len {
                let ch = chars[i];
                if ch.is_ascii_digit() || ch == '_' {
                    i += 1;
                } else if ch == '.' && !seen_dot && !seen_exp {
                    seen_dot = true;
                    i += 1;
                } else if (ch == 'e' || ch == 'E') && !seen_exp && i > pos {
                    // Only consume the `e` when a real exponent follows;
                    // otherwise it belongs to an identifier (`2e` = 2 × e), so
                    // leave it for the identifier scanner.
                    let mut look = i + 1;
                    if look < len && (chars[look] == '+' || chars[look] == '-') {
                        look += 1;
                    }
                    if look >= len || !chars[look].is_ascii_digit() {
                        break;
                    }
                    seen_exp = true;
                    i = look;
                } else {
                    break;
                }
            }
            let mut raw: String = chars[pos..i].iter().filter(|ch| **ch != '_').collect();
            if raw.ends_with('.') {
                raw.push('0');
            }
            let number = if seen_dot || seen_exp {
                Number::Float(
                    raw.parse::<f64>()
                        .map_err(|_| format!("Invalid number '{raw}' at position {}", pos + 1))?,
                )
            } else {
                match raw.parse::<i64>() {
                    Ok(v) => Number::Int(v),
                    // A decimal literal too large for i64 degrades to f64.
                    Err(_) => {
                        Number::Float(raw.parse::<f64>().map_err(|_| {
                            format!("Invalid number '{raw}' at position {}", pos + 1)
                        })?)
                    }
                }
            };
            out.push(Token {
                kind: TokenKind::Number(number),
                pos,
            });
            continue;
        }

        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            out.push(Token {
                kind: TokenKind::Ident(name),
                pos,
            });
            continue;
        }

        // Multi-character operators must be checked before their prefixes.
        let next = chars.get(i + 1).copied();
        let (kind, width) = match c {
            '*' if next == Some('*') => (TokenKind::DoubleStar, 2),
            '^' if next == Some('^') => (TokenKind::DoubleCaret, 2),
            '<' if next == Some('<') => (TokenKind::Shl, 2),
            '>' if next == Some('>') => (TokenKind::Shr, 2),
            '+' => (TokenKind::Plus, 1),
            '-' => (TokenKind::Minus, 1),
            '*' => (TokenKind::Star, 1),
            '/' => (TokenKind::Slash, 1),
            '%' => (TokenKind::Percent, 1),
            '^' => (TokenKind::Caret, 1),
            '!' => (TokenKind::Bang, 1),
            '&' => (TokenKind::Amp, 1),
            '|' => (TokenKind::Pipe, 1),
            '~' => (TokenKind::Tilde, 1),
            '(' => (TokenKind::LParen, 1),
            ')' => (TokenKind::RParen, 1),
            ',' => (TokenKind::Comma, 1),
            _ => {
                return Err(format!(
                    "Unexpected character '{}' at position {}",
                    c,
                    pos + 1
                ))
            }
        };
        i += width;
        out.push(Token { kind, pos });
    }

    Ok(out)
}

// ── AST & parser ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum UnaryOp {
    Plus,
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy)]
enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    And,
    Or,
    Xor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, Copy)]
enum PostfixOp {
    Factorial,
}

#[derive(Debug, Clone)]
enum Expr {
    Num(Number),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    Postfix(PostfixOp, Box<Expr>),
    Call(String, Vec<Expr>),
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: Cell<usize>,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_kind(&self) -> Option<&TokenKind> {
        self.peek().map(|t| &t.kind)
    }

    fn peek_ident(&self, word: &str) -> bool {
        matches!(self.peek_kind(), Some(TokenKind::Ident(name)) if name.eq_ignore_ascii_case(word))
    }

    fn at(&self, kind: &TokenKind) -> bool {
        matches!(self.peek_kind(), Some(actual) if actual == kind)
    }

    fn enter(&self) -> Result<(), String> {
        let depth = self.depth.get();
        if depth >= MAX_DEPTH {
            return Err("expression nesting is too deep".to_string());
        }
        self.depth.set(depth + 1);
        Ok(())
    }

    fn leave(&self) {
        self.depth.set(self.depth.get() - 1);
    }

    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.enter()?;
        let value = self.parse_or()?;
        self.leave();
        Ok(value)
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_xor()?;
        loop {
            if self.at(&TokenKind::Pipe) || self.peek_ident("or") {
                self.pos += 1;
                let right = self.parse_xor()?;
                left = Expr::Binary(BinaryOp::Or, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_xor(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        loop {
            if self.at(&TokenKind::DoubleCaret) || self.peek_ident("xor") {
                self.pos += 1;
                let right = self.parse_and()?;
                left = Expr::Binary(BinaryOp::Xor, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_shift()?;
        loop {
            if self.at(&TokenKind::Amp) || self.peek_ident("and") {
                self.pos += 1;
                let right = self.parse_shift()?;
                left = Expr::Binary(BinaryOp::And, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_sum()?;
        loop {
            let op = if self.at(&TokenKind::Shl) {
                BinaryOp::Shl
            } else if self.at(&TokenKind::Shr) {
                BinaryOp::Shr
            } else {
                break;
            };
            self.pos += 1;
            let right = self.parse_sum()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_sum(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_product()?;
        loop {
            let op = if self.at(&TokenKind::Plus) {
                BinaryOp::Add
            } else if self.at(&TokenKind::Minus) {
                BinaryOp::Sub
            } else {
                break;
            };
            self.pos += 1;
            let right = self.parse_product()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// True when a token can begin an operand (used for implicit multiplication).
    fn starts_operand(&self) -> bool {
        match self.peek_kind() {
            Some(TokenKind::Number(_)) | Some(TokenKind::LParen) => true,
            Some(TokenKind::Ident(name)) => !is_operator_ident(name),
            _ => false,
        }
    }

    fn parse_product(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            let explicit = self.at(&TokenKind::Star)
                || self.at(&TokenKind::Slash)
                || self.at(&TokenKind::Percent);
            let op = if self.at(&TokenKind::Star) {
                BinaryOp::Mul
            } else if self.at(&TokenKind::Slash) {
                BinaryOp::Div
            } else if self.at(&TokenKind::Percent) {
                BinaryOp::Rem
            } else if self.starts_operand() {
                // `2(3+4)`, `2pi`, `(1+2)(3+4)` — an omitted `*`.
                BinaryOp::Mul
            } else {
                break;
            };
            if explicit {
                self.pos += 1;
            }
            let right = self.parse_unary()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        self.enter()?;
        let value = match self.peek_kind().cloned() {
            Some(TokenKind::Plus) => {
                self.pos += 1;
                Expr::Unary(UnaryOp::Plus, Box::new(self.parse_unary()?))
            }
            Some(TokenKind::Minus) => {
                self.pos += 1;
                Expr::Unary(UnaryOp::Neg, Box::new(self.parse_unary()?))
            }
            Some(TokenKind::Tilde) => {
                self.pos += 1;
                Expr::Unary(UnaryOp::Not, Box::new(self.parse_unary()?))
            }
            _ => self.parse_power()?,
        };
        self.leave();
        Ok(value)
    }

    fn parse_power(&mut self) -> Result<Expr, String> {
        self.enter()?;
        let base = self.parse_postfix()?;
        let value = if self.at(&TokenKind::Caret) || self.at(&TokenKind::DoubleStar) {
            self.pos += 1;
            // Parsing the exponent through `unary` gives right-associativity
            // (`2^3^2 = 512`) and accepts a sign (`2^-2 = 0.25`).
            let exponent = self.parse_unary()?;
            Expr::Binary(BinaryOp::Pow, Box::new(base), Box::new(exponent))
        } else {
            base
        };
        self.leave();
        Ok(value)
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        self.enter()?;
        let mut value = self.parse_primary()?;
        while self.at(&TokenKind::Bang) {
            self.pos += 1;
            value = Expr::Postfix(PostfixOp::Factorial, Box::new(value));
        }
        self.leave();
        Ok(value)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        self.enter()?;
        let value = match self.peek_kind().cloned() {
            Some(TokenKind::Number(n)) => {
                self.pos += 1;
                Expr::Num(n)
            }
            Some(TokenKind::Ident(name)) => {
                self.pos += 1;
                let lower = name.to_ascii_lowercase();
                if self.at(&TokenKind::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if !self.at(&TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.at(&TokenKind::Comma) {
                                self.pos += 1;
                                continue;
                            }
                            break;
                        }
                    }
                    if !self.at(&TokenKind::RParen) {
                        return Err(format!("Expected ')' after arguments of '{name}'"));
                    }
                    self.pos += 1;
                    Expr::Call(lower, args)
                } else {
                    match lower.as_str() {
                        "pi" => Expr::Num(Number::Float(std::f64::consts::PI)),
                        "e" => Expr::Num(Number::Float(std::f64::consts::E)),
                        "tau" => Expr::Num(Number::Float(std::f64::consts::TAU)),
                        _ => {
                            return Err(format!(
                                "Unknown identifier '{name}' at position {}",
                                self.tokens[self.pos - 1].pos + 1
                            ))
                        }
                    }
                }
            }
            Some(TokenKind::LParen) => {
                self.pos += 1;
                let inner = self.parse_expr()?;
                if !self.at(&TokenKind::RParen) {
                    return Err("Expected ')'".to_string());
                }
                self.pos += 1;
                inner
            }
            Some(other) => {
                return Err(format!(
                    "Unexpected token {other:?} at position {}",
                    self.tokens[self.pos].pos + 1
                ))
            }
            None => return Err("Unexpected end of expression".to_string()),
        };
        self.leave();
        Ok(value)
    }
}

fn parse(input: &str) -> Result<Expr, String> {
    if input.chars().count() > MAX_INPUT_CHARS {
        return Err(format!(
            "Expression is too long (max {MAX_INPUT_CHARS} characters)"
        ));
    }
    let tokens = tokenize(input)?;
    if tokens.len() > MAX_TOKENS {
        return Err(format!("Expression has too many tokens (max {MAX_TOKENS})"));
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        depth: Cell::new(0),
    };
    let expr = parser.parse_expr()?;
    if let Some(token) = parser.peek() {
        return Err(format!("Unexpected token at position {}", token.pos + 1));
    }
    Ok(expr)
}

// ── Evaluation ───────────────────────────────────────────────────────────────

fn add(a: Number, b: Number) -> Result<Number, String> {
    match (a, b) {
        (Number::Int(x), Number::Int(y)) => Ok(match x.checked_add(y) {
            Some(value) => Number::Int(value),
            None => Number::Float((x as f64) + (y as f64)),
        }),
        _ => finite(Number::Float(to_f64(a) + to_f64(b))),
    }
}

fn sub(a: Number, b: Number) -> Result<Number, String> {
    match (a, b) {
        (Number::Int(x), Number::Int(y)) => Ok(match x.checked_sub(y) {
            Some(value) => Number::Int(value),
            None => Number::Float((x as f64) - (y as f64)),
        }),
        _ => finite(Number::Float(to_f64(a) - to_f64(b))),
    }
}

fn mul(a: Number, b: Number) -> Result<Number, String> {
    match (a, b) {
        (Number::Int(x), Number::Int(y)) => Ok(match x.checked_mul(y) {
            Some(value) => Number::Int(value),
            None => Number::Float((x as f64) * (y as f64)),
        }),
        _ => finite(Number::Float(to_f64(a) * to_f64(b))),
    }
}

fn rem(a: Number, b: Number) -> Result<Number, String> {
    match (a, b) {
        (Number::Int(x), Number::Int(y)) => {
            if y == 0 {
                return Err("Modulo by zero".to_string());
            }
            Ok(match x.checked_rem(y) {
                Some(value) => Number::Int(value),
                None => Number::Float((x as f64) % (y as f64)),
            })
        }
        _ => {
            let divisor = to_f64(b);
            if divisor == 0.0 {
                return Err("Modulo by zero".to_string());
            }
            finite(Number::Float(to_f64(a) % divisor))
        }
    }
}

fn pow(a: Number, b: Number) -> Result<Number, String> {
    let base = to_f64(a);
    let exponent = to_f64(b);
    let value = match b {
        Number::Int(n) if n >= i32::MIN as i64 && n <= i32::MAX as i64 => base.powi(n as i32),
        _ => base.powf(exponent),
    };
    finite(Number::Float(value))
}

fn shift(v: i64, amount: i64, left: bool) -> Result<Number, String> {
    if !(0..=63).contains(&amount) {
        return Err("Shift amount must be between 0 and 63".to_string());
    }
    Ok(Number::Int(if left { v << amount } else { v >> amount }))
}

fn factorial(v: Number) -> Result<Number, String> {
    let n = as_int(v)?;
    if n < 0 {
        return Err("Factorial requires a non-negative integer".to_string());
    }
    if n > 20 {
        return Err(
            "Factorial argument is too large (maximum is 20 for 64-bit integers)".to_string(),
        );
    }
    let mut result: i64 = 1;
    for k in 2..=n {
        result = result
            .checked_mul(k)
            .ok_or_else(|| "Factorial result overflows 64-bit integer".to_string())?;
    }
    Ok(Number::Int(result))
}

fn eval(expr: &Expr) -> Result<Number, String> {
    match expr {
        Expr::Num(n) => Ok(*n),
        Expr::Unary(op, inner) => {
            let value = eval(inner)?;
            match op {
                UnaryOp::Plus => Ok(value),
                UnaryOp::Neg => match value {
                    Number::Int(i) => match i.checked_neg() {
                        Some(negated) => Ok(Number::Int(negated)),
                        None => Ok(Number::Float(-(i as f64))),
                    },
                    Number::Float(f) => Ok(Number::Float(-f)),
                },
                UnaryOp::Not => Ok(Number::Int(!as_int(value)?)),
            }
        }
        Expr::Binary(op, left, right) => {
            let a = eval(left)?;
            let b = eval(right)?;
            match op {
                BinaryOp::Add => add(a, b),
                BinaryOp::Sub => sub(a, b),
                BinaryOp::Mul => mul(a, b),
                BinaryOp::Div => {
                    let divisor = to_f64(b);
                    if divisor == 0.0 {
                        return Err("Division by zero".to_string());
                    }
                    finite(Number::Float(to_f64(a) / divisor))
                }
                BinaryOp::Rem => rem(a, b),
                BinaryOp::Pow => pow(a, b),
                BinaryOp::And => Ok(Number::Int(as_int(a)? & as_int(b)?)),
                BinaryOp::Or => Ok(Number::Int(as_int(a)? | as_int(b)?)),
                BinaryOp::Xor => Ok(Number::Int(as_int(a)? ^ as_int(b)?)),
                BinaryOp::Shl => shift(as_int(a)?, as_int(b)?, true),
                BinaryOp::Shr => shift(as_int(a)?, as_int(b)?, false),
            }
        }
        Expr::Postfix(PostfixOp::Factorial, inner) => factorial(eval(inner)?),
        Expr::Call(name, args) => {
            let values: Vec<Number> = args.iter().map(eval).collect::<Result<_, _>>()?;
            eval_call(name, &values)
        }
    }
}

fn one_arg(values: &[Number], name: &str) -> Result<f64, String> {
    if values.len() != 1 {
        return Err(format!("'{name}' expects 1 argument, got {}", values.len()));
    }
    Ok(to_f64(values[0]))
}

fn two_args(values: &[Number], name: &str) -> Result<(f64, f64), String> {
    if values.len() != 2 {
        return Err(format!(
            "'{name}' expects 2 arguments, got {}",
            values.len()
        ));
    }
    Ok((to_f64(values[0]), to_f64(values[1])))
}

fn eval_call(name: &str, values: &[Number]) -> Result<Number, String> {
    let f = |func: fn(f64) -> f64| -> Result<Number, String> {
        finite(Number::Float(func(one_arg(values, name)?)))
    };
    match name {
        "sqrt" => f(f64::sqrt),
        "abs" => f(f64::abs),
        "exp" => f(f64::exp),
        "ln" => f(f64::ln),
        "log" | "log10" => f(f64::log10),
        "log2" => f(f64::log2),
        "sin" => f(f64::sin),
        "cos" => f(f64::cos),
        "tan" => f(f64::tan),
        "asin" => f(f64::asin),
        "acos" => f(f64::acos),
        "atan" => f(f64::atan),
        "sinh" => f(f64::sinh),
        "cosh" => f(f64::cosh),
        "tanh" => f(f64::tanh),
        "floor" => f(f64::floor),
        "ceil" => f(f64::ceil),
        "round" => f(f64::round),
        "trunc" => f(f64::trunc),
        "sign" => {
            let value = one_arg(values, name)?;
            Ok(Number::Int(if value > 0.0 {
                1
            } else if value < 0.0 {
                -1
            } else {
                0
            }))
        }
        "min" => {
            let (a, b) = two_args(values, name)?;
            Ok(Number::Float(a.min(b)))
        }
        "max" => {
            let (a, b) = two_args(values, name)?;
            Ok(Number::Float(a.max(b)))
        }
        "atan2" => {
            let (y, x) = two_args(values, name)?;
            finite(Number::Float(y.atan2(x)))
        }
        "hypot" => {
            let (a, b) = two_args(values, name)?;
            finite(Number::Float(a.hypot(b)))
        }
        "pow" => {
            let (a, b) = two_args(values, name)?;
            pow(Number::Float(a), Number::Float(b))
        }
        "fact" | "factorial" => {
            if values.len() != 1 {
                return Err(format!("'{name}' expects 1 argument, got {}", values.len()));
            }
            factorial(values[0])
        }
        "not" => {
            if values.len() != 1 {
                return Err(format!("'{name}' expects 1 argument, got {}", values.len()));
            }
            Ok(Number::Int(!as_int(values[0])?))
        }
        "and" | "or" | "xor" | "shl" | "shr" => {
            if values.len() != 2 {
                return Err(format!(
                    "'{name}' expects 2 arguments, got {}",
                    values.len()
                ));
            }
            // Convert the numbers directly (not through f64) so a large i64
            // keeps every bit.
            let x = as_int(values[0])?;
            let y = as_int(values[1])?;
            match name {
                "and" => Ok(Number::Int(x & y)),
                "or" => Ok(Number::Int(x | y)),
                "xor" => Ok(Number::Int(x ^ y)),
                "shl" => shift(x, y, true),
                _ => shift(x, y, false),
            }
        }
        _ => Err(format!("Unknown function '{name}'")),
    }
}

// ── Formatting & tool handler ────────────────────────────────────────────────

fn format_number(number: Number, precision: Option<usize>) -> String {
    match number {
        Number::Int(i) => i.to_string(),
        Number::Float(f) => {
            // `-0.0` is the only float that would print with a misleading
            // sign; every arithmetic route that produces it means zero.
            let f = if f == 0.0 { 0.0 } else { f };
            let rounded = match precision {
                Some(places) => {
                    let factor = 10f64.powi(places as i32);
                    let scaled = f * factor;
                    if scaled.is_finite() {
                        scaled.round() / factor
                    } else {
                        f
                    }
                }
                None => f,
            };
            let text = rounded.to_string();
            if text.contains('.') {
                text.trim_end_matches('0').trim_end_matches('.').to_string()
            } else {
                text
            }
        }
    }
}

/// Entry point called by the tool dispatcher with the already-parsed arguments.
pub(crate) fn handle(args: &Value) -> String {
    let expression = args["expression"].as_str().unwrap_or("").trim();
    if expression.is_empty() {
        return "Error: 'expression' is required.".to_string();
    }

    let precision = args["precision"]
        .as_u64()
        .or_else(|| {
            args["precision"]
                .as_str()
                .and_then(|raw| raw.trim().parse::<u64>().ok())
        })
        .map(|value| value.min(15) as usize);

    match parse(expression).and_then(|ast| eval(&ast)) {
        Ok(number) => {
            let result = match number {
                Number::Int(i) => json!(i),
                Number::Float(f) => json!(f),
            };
            let payload = json!({
                "expression": expression,
                "result": result,
                "formatted": format_number(number, precision),
            });
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
        }
        Err(err) => format!("Error: {err}"),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn evaluate(input: &str) -> String {
        let ast = parse(input).unwrap_or_else(|err| panic!("parse({input:?}): {err}"));
        let number = eval(&ast).unwrap_or_else(|err| panic!("eval({input:?}): {err}"));
        format_number(number, None)
    }

    fn assert_eval(input: &str, expected: &str) {
        assert_eq!(evaluate(input), expected, "expression: {input}");
    }

    fn assert_near(input: &str, expected: f64) {
        let actual: f64 = evaluate(input).parse().unwrap();
        assert!(
            (actual - expected).abs() < 1e-12,
            "{input}: expected {expected}, got {actual}"
        );
    }

    fn assert_error(input: &str) {
        let ast = match parse(input) {
            Ok(ast) => ast,
            Err(_) => return,
        };
        assert!(eval(&ast).is_err(), "expected an error for {input}");
    }

    #[test]
    fn number_literals_and_scientific_notation() {
        assert_eval("1.5e3", "1500");
        assert_eval("1.5e-3", "0.0015");
        assert_eval("1e5+1", "100001");
        // An `e` without an exponent is the constant, not a broken literal.
        assert_near("2e", 2.0 * std::f64::consts::E);
        assert_eval("0xFF", "255");
        assert_eval("0b1111", "15");
        assert_eval("0o10", "8");
        assert_eval("1_000_000", "1000000");
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eval("1+2*3", "7");
        assert_eval("(1+2)*3", "9");
        assert_eval("10-2-3", "5");
        assert_eval("10/4", "2.5");
        assert_eval("10%3", "1");
        assert_eval("10.5%3", "1.5");
        assert_eval("1.5e3+0.5", "1500.5");
        assert_eval(".5*4", "2");
        assert_eval("-3+5", "2");
        assert_eval("--5", "5");
    }

    #[test]
    fn exponentiation_is_right_associative_and_signed() {
        assert_eval("2^10", "1024");
        assert_eval("2**8", "256");
        assert_eval("2^3^2", "512");
        assert_eval("-2^2", "-4");
        assert_eval("(-2)^2", "4");
        assert_eval("2^-2", "0.25");
        assert_near("9^0.5", 3.0);
        assert_eval("5^0", "1");
    }

    #[test]
    fn factorial_and_bitwise() {
        assert_eval("5!", "120");
        assert_eval("0!", "1");
        assert_eval("2^3!", "64");
        assert_eval("3!^2", "36");
        assert_eval("0xFF+0b1010", "265");
        assert_eval("0xFF & 0b1111", "15");
        assert_eval("0xF0 | 0x0F", "255");
        assert_eval("5 xor 3", "6");
        assert_eval("5 ^^ 3", "6");
        assert_eval("~0", "-1");
        assert_eval("1 << 4", "16");
        assert_eval("256 >> 4", "16");
        assert_eval("0o17", "15");
        assert_eval("1_000+2_000", "3000");
    }

    #[test]
    fn constants_functions_and_implicit_multiplication() {
        assert_near("pi", std::f64::consts::PI);
        assert_near("tau", std::f64::consts::TAU);
        assert_eval("sqrt(16)", "4");
        assert_eval("abs(-7)", "7");
        assert_eval("max(1,9)", "9");
        assert_eval("pow(2,8)", "256");
        assert_eval("min(3,1)+2", "3");
        assert_eval("2(3+4)", "14");
        assert_eval("(1+2)(3+4)", "21");
        assert_near("2pi", 2.0 * std::f64::consts::PI);
        assert_eval("xor(0xF0,0xFF)", "15");
    }

    #[test]
    fn booleans_follow_bitwise_precedence() {
        assert_eval("1 | 2 xor 3 & 4", "3");
        assert_eval("1 | 6 xor 3 & 2", "5");
        assert_eval("2 and 3 or 4", "6");
    }

    #[test]
    fn domain_and_syntax_errors() {
        assert_error("1/0");
        assert_error("1%0");
        assert_error("sqrt(-1)");
        assert_error("log(0)");
        assert_error("5 & 1.5");
        assert_error("(-1)!");
        assert_error("21!");
        assert_error("1 << 64");
        assert_error("abc");
        assert_error("nosuch(1)");
        assert_error("sqrt(1,2)");
        assert_error("1 +");
        assert_error("(1+2");
        assert_error("0b102");
        assert_error("2^^^3");
    }

    #[test]
    fn precision_and_handler() {
        assert_eq!(format_number(Number::Float(6.25), None), "6.25");
        assert_eq!(format_number(Number::Float(6.25), Some(1)), "6.3");
        assert_eq!(format_number(Number::Float(6.0), None), "6");
        let args = json!({"expression": "(2+3)^2/4"});
        let result: Value = serde_json::from_str(&handle(&args)).unwrap();
        assert_eq!(result["result"], json!(6.25));
        assert_eq!(result["formatted"], json!("6.25"));
        assert!(handle(&json!({"expression": "1/0"})).starts_with("Error:"));
    }
}
