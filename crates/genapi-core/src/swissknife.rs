//! SwissKnife expression parser and evaluator.
//!
//! Supports the GenICam SwissKnife expression language including:
//! - Arithmetic: `+ - * / %` and `**` for power
//! - Comparison: `< <= > >= == !=`
//! - Logical: `&& || !`
//! - Bitwise: `& | ^ ~ << >>`
//! - Ternary: `condition ? then : else`
//! - Functions: `sin cos tan asin acos atan atan2 sqrt abs ceil floor round
//!              log ln exp pow min max sgn neg`

use std::collections::HashSet;
use std::fmt;

/// Parsed SwissKnife expression represented as an abstract syntax tree.
#[derive(Debug, Clone)]
pub enum AstNode {
    /// Numeric literal stored as `f64`.
    Number(f64),
    /// Variable lookup resolved at evaluation time.
    Variable(String),
    /// Unary operator applied to a sub-expression.
    Unary {
        /// Operator kind.
        op: UnaryOp,
        /// Operand expression.
        expr: Box<AstNode>,
    },
    /// Binary operator combining two sub-expressions.
    Binary {
        /// Operator kind.
        op: BinaryOp,
        /// Left-hand side operand.
        left: Box<AstNode>,
        /// Right-hand side operand.
        right: Box<AstNode>,
    },
    /// Ternary conditional: `condition ? then_expr : else_expr`.
    Ternary {
        /// Condition expression (non-zero is truthy).
        cond: Box<AstNode>,
        /// Expression evaluated when condition is truthy.
        then_expr: Box<AstNode>,
        /// Expression evaluated when condition is falsy.
        else_expr: Box<AstNode>,
    },
    /// Function call with arguments.
    FnCall {
        /// Function name.
        name: String,
        /// Arguments to the function.
        args: Vec<AstNode>,
    },
}

/// Binary operator kinds supported by the SwissKnife expression language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    // Comparison
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    // Logical
    And,
    Or,
    // Bitwise
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

/// Unary operator kinds supported by the SwissKnife expression language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Plus,
    Minus,
    Not,
    BitNot,
}

/// Error produced while parsing a SwissKnife expression.
#[derive(Debug, Clone)]
pub struct ParseError {
    msg: String,
}

impl ParseError {
    fn new<S: Into<String>>(msg: S) -> Self {
        Self { msg: msg.into() }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl std::error::Error for ParseError {}

/// Error produced while evaluating a SwissKnife expression.
#[derive(Debug, Clone)]
pub enum EvalError {
    /// Variable referenced by the expression has no bound value.
    UnknownVariable(String),
    /// Division by zero occurred.
    DivisionByZero,
    /// Unknown function name.
    UnknownFunction(String),
    /// Wrong number of arguments to function.
    ArityMismatch {
        name: String,
        expected: usize,
        got: usize,
    },
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvalError::UnknownVariable(var) => write!(f, "unknown variable {var}"),
            EvalError::DivisionByZero => write!(f, "division by zero"),
            EvalError::UnknownFunction(name) => write!(f, "unknown function {name}"),
            EvalError::ArityMismatch {
                name,
                expected,
                got,
            } => {
                write!(f, "function {name} expects {expected} args, got {got}")
            }
        }
    }
}

impl std::error::Error for EvalError {}

/// Parse a SwissKnife expression into an [`AstNode`].
pub fn parse_expression(input: &str) -> Result<AstNode, ParseError> {
    let mut parser = Parser::new(input)?;
    let expr = parser.parse_ternary()?;
    if !matches!(parser.lookahead, Token::End) {
        return Err(ParseError::new("unexpected trailing tokens"));
    }
    Ok(expr)
}

/// Evaluate an [`AstNode`] using the provided variable resolver.
///
/// The resolver receives variable identifiers and must return their numeric
/// value. Returning [`EvalError::UnknownVariable`] is propagated to the caller.
pub fn evaluate(
    ast: &AstNode,
    vars: &mut dyn FnMut(&str) -> Result<f64, EvalError>,
) -> Result<f64, EvalError> {
    match ast {
        AstNode::Number(value) => Ok(*value),
        AstNode::Variable(name) => vars(name),
        AstNode::Unary { op, expr } => {
            let inner = evaluate(expr, vars)?;
            match op {
                UnaryOp::Plus => Ok(inner),
                UnaryOp::Minus => Ok(-inner),
                UnaryOp::Not => Ok(if inner == 0.0 { 1.0 } else { 0.0 }),
                UnaryOp::BitNot => Ok(!(inner as i64) as f64),
            }
        }
        AstNode::Binary { op, left, right } => {
            // Short-circuit evaluation for logical operators
            match op {
                BinaryOp::And => {
                    let lhs = evaluate(left, vars)?;
                    if lhs == 0.0 {
                        return Ok(0.0);
                    }
                    let rhs = evaluate(right, vars)?;
                    Ok(if rhs != 0.0 { 1.0 } else { 0.0 })
                }
                BinaryOp::Or => {
                    let lhs = evaluate(left, vars)?;
                    if lhs != 0.0 {
                        return Ok(1.0);
                    }
                    let rhs = evaluate(right, vars)?;
                    Ok(if rhs != 0.0 { 1.0 } else { 0.0 })
                }
                _ => {
                    let lhs = evaluate(left, vars)?;
                    let rhs = evaluate(right, vars)?;
                    eval_binary(*op, lhs, rhs)
                }
            }
        }
        AstNode::Ternary {
            cond,
            then_expr,
            else_expr,
        } => {
            let cond_val = evaluate(cond, vars)?;
            if cond_val != 0.0 {
                evaluate(then_expr, vars)
            } else {
                evaluate(else_expr, vars)
            }
        }
        AstNode::FnCall { name, args } => {
            let evaluated: Result<Vec<f64>, _> = args.iter().map(|a| evaluate(a, vars)).collect();
            let arg_vals = evaluated?;
            eval_function(name, &arg_vals)
        }
    }
}

fn eval_binary(op: BinaryOp, lhs: f64, rhs: f64) -> Result<f64, EvalError> {
    Ok(match op {
        BinaryOp::Add => lhs + rhs,
        BinaryOp::Sub => lhs - rhs,
        BinaryOp::Mul => lhs * rhs,
        BinaryOp::Div => {
            if rhs == 0.0 {
                return Err(EvalError::DivisionByZero);
            }
            lhs / rhs
        }
        BinaryOp::Mod => {
            if rhs == 0.0 {
                return Err(EvalError::DivisionByZero);
            }
            lhs % rhs
        }
        BinaryOp::Pow => lhs.powf(rhs),
        BinaryOp::Lt => {
            if lhs < rhs {
                1.0
            } else {
                0.0
            }
        }
        BinaryOp::Le => {
            if lhs <= rhs {
                1.0
            } else {
                0.0
            }
        }
        BinaryOp::Gt => {
            if lhs > rhs {
                1.0
            } else {
                0.0
            }
        }
        BinaryOp::Ge => {
            if lhs >= rhs {
                1.0
            } else {
                0.0
            }
        }
        BinaryOp::Eq => {
            if (lhs - rhs).abs() < f64::EPSILON {
                1.0
            } else {
                0.0
            }
        }
        BinaryOp::Ne => {
            if (lhs - rhs).abs() >= f64::EPSILON {
                1.0
            } else {
                0.0
            }
        }
        BinaryOp::And | BinaryOp::Or => unreachable!("handled by short-circuit"),
        BinaryOp::BitAnd => ((lhs as i64) & (rhs as i64)) as f64,
        BinaryOp::BitOr => ((lhs as i64) | (rhs as i64)) as f64,
        BinaryOp::BitXor => ((lhs as i64) ^ (rhs as i64)) as f64,
        BinaryOp::Shl => ((lhs as i64) << (rhs as u32)) as f64,
        BinaryOp::Shr => ((lhs as i64) >> (rhs as u32)) as f64,
    })
}

fn eval_function(name: &str, args: &[f64]) -> Result<f64, EvalError> {
    // Normalize function name to lowercase for matching
    let name_lower = name.to_ascii_lowercase();

    match name_lower.as_str() {
        // Single-argument functions
        "sin" => expect_args(name, args, 1).map(|a| a[0].sin()),
        "cos" => expect_args(name, args, 1).map(|a| a[0].cos()),
        "tan" => expect_args(name, args, 1).map(|a| a[0].tan()),
        "asin" => expect_args(name, args, 1).map(|a| a[0].asin()),
        "acos" => expect_args(name, args, 1).map(|a| a[0].acos()),
        "atan" => expect_args(name, args, 1).map(|a| a[0].atan()),
        "sqrt" => expect_args(name, args, 1).map(|a| a[0].sqrt()),
        "abs" => expect_args(name, args, 1).map(|a| a[0].abs()),
        "ceil" => expect_args(name, args, 1).map(|a| a[0].ceil()),
        "floor" => expect_args(name, args, 1).map(|a| a[0].floor()),
        "round" => expect_args(name, args, 1).map(|a| a[0].round()),
        "trunc" => expect_args(name, args, 1).map(|a| a[0].trunc()),
        "ln" => expect_args(name, args, 1).map(|a| a[0].ln()),
        "log" => expect_args(name, args, 1).map(|a| a[0].log10()),
        "log10" => expect_args(name, args, 1).map(|a| a[0].log10()),
        "log2" => expect_args(name, args, 1).map(|a| a[0].log2()),
        "exp" => expect_args(name, args, 1).map(|a| a[0].exp()),
        "neg" => expect_args(name, args, 1).map(|a| -a[0]),
        "sgn" | "sign" => expect_args(name, args, 1).map(|a| {
            if a[0] > 0.0 {
                1.0
            } else if a[0] < 0.0 {
                -1.0
            } else {
                0.0
            }
        }),
        "e" => expect_args(name, args, 0).map(|_| std::f64::consts::E),
        "pi" => expect_args(name, args, 0).map(|_| std::f64::consts::PI),

        // Two-argument functions
        "atan2" => expect_args(name, args, 2).map(|a| a[0].atan2(a[1])),
        "pow" => expect_args(name, args, 2).map(|a| a[0].powf(a[1])),
        "min" => expect_args(name, args, 2).map(|a| a[0].min(a[1])),
        "max" => expect_args(name, args, 2).map(|a| a[0].max(a[1])),
        "fmod" => expect_args(name, args, 2).map(|a| a[0] % a[1]),

        _ => Err(EvalError::UnknownFunction(name.to_string())),
    }
}

fn expect_args<'a>(name: &str, args: &'a [f64], expected: usize) -> Result<&'a [f64], EvalError> {
    if args.len() != expected {
        Err(EvalError::ArityMismatch {
            name: name.to_string(),
            expected,
            got: args.len(),
        })
    } else {
        Ok(args)
    }
}

/// Collect all variable identifiers referenced by the AST.
pub fn collect_identifiers(ast: &AstNode, out: &mut HashSet<String>) {
    match ast {
        AstNode::Number(_) => {}
        AstNode::Variable(name) => {
            out.insert(name.clone());
        }
        AstNode::Unary { expr, .. } => collect_identifiers(expr, out),
        AstNode::Binary { left, right, .. } => {
            collect_identifiers(left, out);
            collect_identifiers(right, out);
        }
        AstNode::Ternary {
            cond,
            then_expr,
            else_expr,
        } => {
            collect_identifiers(cond, out);
            collect_identifiers(then_expr, out);
            collect_identifiers(else_expr, out);
        }
        AstNode::FnCall { args, .. } => {
            for arg in args {
                collect_identifiers(arg, out);
            }
        }
    }
}

// ============================================================================
// Lexer
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Ident(String),
    // Arithmetic
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    StarStar, // **
    // Comparison
    Lt,
    Le,
    Gt,
    Ge,
    EqEq,
    Ne,
    // Logical
    AmpAmp,
    PipePipe,
    Bang,
    // Bitwise
    Amp,
    Pipe,
    Caret,
    Tilde,
    LtLt,
    GtGt,
    // Ternary
    Question,
    Colon,
    // Grouping
    LParen,
    RParen,
    Comma,
    End,
}

struct Lexer<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str) -> Self {
        Lexer {
            input: input.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<u8> {
        self.input.get(self.pos + 1).copied()
    }

    fn advance_by(&mut self, n: usize) {
        self.pos += n;
    }

    fn next_token(&mut self) -> Result<Token, ParseError> {
        self.skip_ws();
        let Some(byte) = self.peek() else {
            return Ok(Token::End);
        };

        match byte {
            b'0'..=b'9' | b'.' => self.lex_number(),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.lex_ident(),
            b'+' => {
                self.advance_by(1);
                Ok(Token::Plus)
            }
            b'-' => {
                self.advance_by(1);
                Ok(Token::Minus)
            }
            b'*' => {
                if self.peek_next() == Some(b'*') {
                    self.advance_by(2);
                    Ok(Token::StarStar)
                } else {
                    self.advance_by(1);
                    Ok(Token::Star)
                }
            }
            b'/' => {
                self.advance_by(1);
                Ok(Token::Slash)
            }
            b'%' => {
                self.advance_by(1);
                Ok(Token::Percent)
            }
            b'<' => match self.peek_next() {
                Some(b'=') => {
                    self.advance_by(2);
                    Ok(Token::Le)
                }
                Some(b'<') => {
                    self.advance_by(2);
                    Ok(Token::LtLt)
                }
                _ => {
                    self.advance_by(1);
                    Ok(Token::Lt)
                }
            },
            b'>' => match self.peek_next() {
                Some(b'=') => {
                    self.advance_by(2);
                    Ok(Token::Ge)
                }
                Some(b'>') => {
                    self.advance_by(2);
                    Ok(Token::GtGt)
                }
                _ => {
                    self.advance_by(1);
                    Ok(Token::Gt)
                }
            },
            b'=' => {
                if self.peek_next() == Some(b'=') {
                    self.advance_by(2);
                    Ok(Token::EqEq)
                } else {
                    Err(ParseError::new("unexpected '=' (use '==' for equality)"))
                }
            }
            b'!' => {
                if self.peek_next() == Some(b'=') {
                    self.advance_by(2);
                    Ok(Token::Ne)
                } else {
                    self.advance_by(1);
                    Ok(Token::Bang)
                }
            }
            b'&' => {
                if self.peek_next() == Some(b'&') {
                    self.advance_by(2);
                    Ok(Token::AmpAmp)
                } else {
                    self.advance_by(1);
                    Ok(Token::Amp)
                }
            }
            b'|' => {
                if self.peek_next() == Some(b'|') {
                    self.advance_by(2);
                    Ok(Token::PipePipe)
                } else {
                    self.advance_by(1);
                    Ok(Token::Pipe)
                }
            }
            b'^' => {
                self.advance_by(1);
                Ok(Token::Caret)
            }
            b'~' => {
                self.advance_by(1);
                Ok(Token::Tilde)
            }
            b'?' => {
                self.advance_by(1);
                Ok(Token::Question)
            }
            b':' => {
                self.advance_by(1);
                Ok(Token::Colon)
            }
            b'(' => {
                self.advance_by(1);
                Ok(Token::LParen)
            }
            b')' => {
                self.advance_by(1);
                Ok(Token::RParen)
            }
            b',' => {
                self.advance_by(1);
                Ok(Token::Comma)
            }
            _ => Err(ParseError::new(format!(
                "unexpected character '{}'",
                byte as char
            ))),
        }
    }

    fn skip_ws(&mut self) {
        while let Some(byte) = self.peek() {
            if byte.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn lex_number(&mut self) -> Result<Token, ParseError> {
        let start = self.pos;

        // Check for hex literal: 0x or 0X
        if self.peek() == Some(b'0') {
            let next = self.input.get(self.pos + 1).copied();
            if next == Some(b'x') || next == Some(b'X') {
                self.pos += 2; // skip "0x"
                let hex_start = self.pos;
                while let Some(b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F') = self.peek() {
                    self.pos += 1;
                }
                if self.pos == hex_start {
                    return Err(ParseError::new("hex literal has no digits"));
                }
                let hex_text = std::str::from_utf8(&self.input[hex_start..self.pos])
                    .map_err(|_| ParseError::new("invalid UTF-8 in hex literal"))?;
                let value = u64::from_str_radix(hex_text, 16)
                    .map_err(|_| ParseError::new(format!("invalid hex literal: 0x{hex_text}")))?;
                return Ok(Token::Number(value as f64));
            }
        }

        let mut seen_digit = false;
        let mut seen_dot = false;
        let mut seen_exp = false;

        while let Some(byte) = self.peek() {
            match byte {
                b'0'..=b'9' => {
                    seen_digit = true;
                    self.pos += 1;
                }
                b'.' if !seen_dot && !seen_exp => {
                    seen_dot = true;
                    self.pos += 1;
                }
                b'e' | b'E' if !seen_exp && seen_digit => {
                    seen_exp = true;
                    self.pos += 1;
                    // Optional sign after exponent
                    if let Some(b'+' | b'-') = self.peek() {
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
        if !seen_digit {
            return Err(ParseError::new("invalid number literal"));
        }
        let slice = &self.input[start..self.pos];
        let text =
            std::str::from_utf8(slice).map_err(|_| ParseError::new("invalid UTF-8 in number"))?;
        let value = text
            .parse::<f64>()
            .map_err(|_| ParseError::new(format!("failed to parse number: {text}")))?;
        Ok(Token::Number(value))
    }

    fn lex_ident(&mut self) -> Result<Token, ParseError> {
        let start = self.pos;
        self.pos += 1;
        while let Some(byte) = self.peek() {
            if byte.is_ascii_alphanumeric() || byte == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let slice = &self.input[start..self.pos];
        let text = std::str::from_utf8(slice)
            .map_err(|_| ParseError::new("invalid UTF-8 in identifier"))?;
        Ok(Token::Ident(text.to_string()))
    }
}

// ============================================================================
// Parser - Operator Precedence (lowest to highest):
// 1. Ternary: ?:
// 2. Logical OR: ||
// 3. Logical AND: &&
// 4. Bitwise OR: |
// 5. Bitwise XOR: ^
// 6. Bitwise AND: &
// 7. Equality: == !=
// 8. Comparison: < <= > >=
// 9. Shift: << >>
// 10. Additive: + -
// 11. Multiplicative: * / %
// 12. Power: **
// 13. Unary: + - ! ~
// 14. Primary: numbers, identifiers, function calls, (expr)
// ============================================================================

struct Parser<'a> {
    lexer: Lexer<'a>,
    lookahead: Token,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Result<Self, ParseError> {
        let mut lexer = Lexer::new(input);
        let lookahead = lexer.next_token()?;
        Ok(Parser { lexer, lookahead })
    }

    fn advance(&mut self) -> Result<(), ParseError> {
        self.lookahead = self.lexer.next_token()?;
        Ok(())
    }

    // Level 1: Ternary
    fn parse_ternary(&mut self) -> Result<AstNode, ParseError> {
        let cond = self.parse_or()?;
        if matches!(self.lookahead, Token::Question) {
            self.advance()?;
            let then_expr = self.parse_ternary()?;
            if !matches!(self.lookahead, Token::Colon) {
                return Err(ParseError::new("expected ':' in ternary expression"));
            }
            self.advance()?;
            let else_expr = self.parse_ternary()?;
            Ok(AstNode::Ternary {
                cond: Box::new(cond),
                then_expr: Box::new(then_expr),
                else_expr: Box::new(else_expr),
            })
        } else {
            Ok(cond)
        }
    }

    // Level 2: Logical OR
    fn parse_or(&mut self) -> Result<AstNode, ParseError> {
        let mut node = self.parse_and()?;
        while matches!(self.lookahead, Token::PipePipe) {
            self.advance()?;
            let rhs = self.parse_and()?;
            node = AstNode::Binary {
                op: BinaryOp::Or,
                left: Box::new(node),
                right: Box::new(rhs),
            };
        }
        Ok(node)
    }

    // Level 3: Logical AND
    fn parse_and(&mut self) -> Result<AstNode, ParseError> {
        let mut node = self.parse_bitor()?;
        while matches!(self.lookahead, Token::AmpAmp) {
            self.advance()?;
            let rhs = self.parse_bitor()?;
            node = AstNode::Binary {
                op: BinaryOp::And,
                left: Box::new(node),
                right: Box::new(rhs),
            };
        }
        Ok(node)
    }

    // Level 4: Bitwise OR
    fn parse_bitor(&mut self) -> Result<AstNode, ParseError> {
        let mut node = self.parse_bitxor()?;
        while matches!(self.lookahead, Token::Pipe) {
            self.advance()?;
            let rhs = self.parse_bitxor()?;
            node = AstNode::Binary {
                op: BinaryOp::BitOr,
                left: Box::new(node),
                right: Box::new(rhs),
            };
        }
        Ok(node)
    }

    // Level 5: Bitwise XOR
    fn parse_bitxor(&mut self) -> Result<AstNode, ParseError> {
        let mut node = self.parse_bitand()?;
        while matches!(self.lookahead, Token::Caret) {
            self.advance()?;
            let rhs = self.parse_bitand()?;
            node = AstNode::Binary {
                op: BinaryOp::BitXor,
                left: Box::new(node),
                right: Box::new(rhs),
            };
        }
        Ok(node)
    }

    // Level 6: Bitwise AND
    fn parse_bitand(&mut self) -> Result<AstNode, ParseError> {
        let mut node = self.parse_equality()?;
        while matches!(self.lookahead, Token::Amp) {
            self.advance()?;
            let rhs = self.parse_equality()?;
            node = AstNode::Binary {
                op: BinaryOp::BitAnd,
                left: Box::new(node),
                right: Box::new(rhs),
            };
        }
        Ok(node)
    }

    // Level 7: Equality
    fn parse_equality(&mut self) -> Result<AstNode, ParseError> {
        let mut node = self.parse_comparison()?;
        loop {
            let op = match &self.lookahead {
                Token::EqEq => BinaryOp::Eq,
                Token::Ne => BinaryOp::Ne,
                _ => break,
            };
            self.advance()?;
            let rhs = self.parse_comparison()?;
            node = AstNode::Binary {
                op,
                left: Box::new(node),
                right: Box::new(rhs),
            };
        }
        Ok(node)
    }

    // Level 8: Comparison
    fn parse_comparison(&mut self) -> Result<AstNode, ParseError> {
        let mut node = self.parse_shift()?;
        loop {
            let op = match &self.lookahead {
                Token::Lt => BinaryOp::Lt,
                Token::Le => BinaryOp::Le,
                Token::Gt => BinaryOp::Gt,
                Token::Ge => BinaryOp::Ge,
                _ => break,
            };
            self.advance()?;
            let rhs = self.parse_shift()?;
            node = AstNode::Binary {
                op,
                left: Box::new(node),
                right: Box::new(rhs),
            };
        }
        Ok(node)
    }

    // Level 9: Shift
    fn parse_shift(&mut self) -> Result<AstNode, ParseError> {
        let mut node = self.parse_additive()?;
        loop {
            let op = match &self.lookahead {
                Token::LtLt => BinaryOp::Shl,
                Token::GtGt => BinaryOp::Shr,
                _ => break,
            };
            self.advance()?;
            let rhs = self.parse_additive()?;
            node = AstNode::Binary {
                op,
                left: Box::new(node),
                right: Box::new(rhs),
            };
        }
        Ok(node)
    }

    // Level 10: Additive
    fn parse_additive(&mut self) -> Result<AstNode, ParseError> {
        let mut node = self.parse_multiplicative()?;
        loop {
            let op = match &self.lookahead {
                Token::Plus => BinaryOp::Add,
                Token::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.advance()?;
            let rhs = self.parse_multiplicative()?;
            node = AstNode::Binary {
                op,
                left: Box::new(node),
                right: Box::new(rhs),
            };
        }
        Ok(node)
    }

    // Level 11: Multiplicative
    fn parse_multiplicative(&mut self) -> Result<AstNode, ParseError> {
        let mut node = self.parse_power()?;
        loop {
            let op = match &self.lookahead {
                Token::Star => BinaryOp::Mul,
                Token::Slash => BinaryOp::Div,
                Token::Percent => BinaryOp::Mod,
                _ => break,
            };
            self.advance()?;
            let rhs = self.parse_power()?;
            node = AstNode::Binary {
                op,
                left: Box::new(node),
                right: Box::new(rhs),
            };
        }
        Ok(node)
    }

    // Level 12: Power (right-associative)
    fn parse_power(&mut self) -> Result<AstNode, ParseError> {
        let base = self.parse_unary()?;
        if matches!(self.lookahead, Token::StarStar) {
            self.advance()?;
            let exp = self.parse_power()?; // Right-associative
            Ok(AstNode::Binary {
                op: BinaryOp::Pow,
                left: Box::new(base),
                right: Box::new(exp),
            })
        } else {
            Ok(base)
        }
    }

    // Level 13: Unary
    fn parse_unary(&mut self) -> Result<AstNode, ParseError> {
        match &self.lookahead {
            Token::Plus => {
                self.advance()?;
                let expr = self.parse_unary()?;
                Ok(AstNode::Unary {
                    op: UnaryOp::Plus,
                    expr: Box::new(expr),
                })
            }
            Token::Minus => {
                self.advance()?;
                let expr = self.parse_unary()?;
                Ok(AstNode::Unary {
                    op: UnaryOp::Minus,
                    expr: Box::new(expr),
                })
            }
            Token::Bang => {
                self.advance()?;
                let expr = self.parse_unary()?;
                Ok(AstNode::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                })
            }
            Token::Tilde => {
                self.advance()?;
                let expr = self.parse_unary()?;
                Ok(AstNode::Unary {
                    op: UnaryOp::BitNot,
                    expr: Box::new(expr),
                })
            }
            _ => self.parse_primary(),
        }
    }

    // Level 14: Primary
    fn parse_primary(&mut self) -> Result<AstNode, ParseError> {
        match self.lookahead.clone() {
            Token::Number(value) => {
                self.advance()?;
                Ok(AstNode::Number(value))
            }
            Token::Ident(name) => {
                self.advance()?;
                // Check for function call
                if matches!(self.lookahead, Token::LParen) {
                    self.advance()?;
                    let mut args = Vec::new();
                    if !matches!(self.lookahead, Token::RParen) {
                        args.push(self.parse_ternary()?);
                        while matches!(self.lookahead, Token::Comma) {
                            self.advance()?;
                            args.push(self.parse_ternary()?);
                        }
                    }
                    if !matches!(self.lookahead, Token::RParen) {
                        return Err(ParseError::new("expected ')' after function arguments"));
                    }
                    self.advance()?;
                    Ok(AstNode::FnCall { name, args })
                } else {
                    Ok(AstNode::Variable(name))
                }
            }
            Token::LParen => {
                self.advance()?;
                let expr = self.parse_ternary()?;
                if !matches!(self.lookahead, Token::RParen) {
                    return Err(ParseError::new("missing closing ')'"));
                }
                self.advance()?;
                Ok(expr)
            }
            Token::End => Err(ParseError::new("unexpected end of expression")),
            other => Err(ParseError::new(format!("unexpected token {other:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval_expr(expr: &str, vars: &[(&str, f64)]) -> f64 {
        let ast = parse_expression(expr).expect("parse failed");
        let mut resolver = |name: &str| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| *v)
                .ok_or_else(|| EvalError::UnknownVariable(name.to_string()))
        };
        evaluate(&ast, &mut resolver).expect("eval failed")
    }

    #[test]
    fn basic_arithmetic() {
        assert!((eval_expr("(A + 2) * 3 - B / 4", &[("A", 4.0), ("B", 8.0)]) - 16.0).abs() < 1e-6);
        assert!((eval_expr("-A + 10 / (B - 5)", &[("A", 3.0), ("B", 7.0)]) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn comparisons() {
        assert_eq!(eval_expr("5 < 10", &[]), 1.0);
        assert_eq!(eval_expr("5 > 10", &[]), 0.0);
        assert_eq!(eval_expr("5 <= 5", &[]), 1.0);
        assert_eq!(eval_expr("5 >= 6", &[]), 0.0);
        assert_eq!(eval_expr("5 == 5", &[]), 1.0);
        assert_eq!(eval_expr("5 != 5", &[]), 0.0);
        assert_eq!(eval_expr("A < B", &[("A", 3.0), ("B", 5.0)]), 1.0);
    }

    #[test]
    fn ternary_expression() {
        assert_eq!(eval_expr("1 ? 10 : 20", &[]), 10.0);
        assert_eq!(eval_expr("0 ? 10 : 20", &[]), 20.0);
        assert_eq!(eval_expr("A > 5 ? A : 5", &[("A", 3.0)]), 5.0);
        assert_eq!(eval_expr("A > 5 ? A : 5", &[("A", 10.0)]), 10.0);
        // Nested ternary
        assert_eq!(
            eval_expr("A < 0 ? -1 : A > 0 ? 1 : 0", &[("A", -5.0)]),
            -1.0
        );
        assert_eq!(eval_expr("A < 0 ? -1 : A > 0 ? 1 : 0", &[("A", 5.0)]), 1.0);
        assert_eq!(eval_expr("A < 0 ? -1 : A > 0 ? 1 : 0", &[("A", 0.0)]), 0.0);
    }

    #[test]
    fn logical_operators() {
        assert_eq!(eval_expr("1 && 1", &[]), 1.0);
        assert_eq!(eval_expr("1 && 0", &[]), 0.0);
        assert_eq!(eval_expr("0 || 1", &[]), 1.0);
        assert_eq!(eval_expr("0 || 0", &[]), 0.0);
        assert_eq!(eval_expr("!0", &[]), 1.0);
        assert_eq!(eval_expr("!1", &[]), 0.0);
        assert_eq!(eval_expr("!5", &[]), 0.0);
    }

    #[test]
    fn bitwise_operators() {
        assert_eq!(eval_expr("5 & 3", &[]), 1.0); // 101 & 011 = 001
        assert_eq!(eval_expr("5 | 3", &[]), 7.0); // 101 | 011 = 111
        assert_eq!(eval_expr("5 ^ 3", &[]), 6.0); // 101 ^ 011 = 110
        assert_eq!(eval_expr("1 << 3", &[]), 8.0);
        assert_eq!(eval_expr("8 >> 2", &[]), 2.0);
    }

    #[test]
    fn hex_literals() {
        assert_eq!(eval_expr("0xFF", &[]), 255.0);
        assert_eq!(eval_expr("0x10", &[]), 16.0);
        assert_eq!(eval_expr("0x0", &[]), 0.0);
        assert_eq!(eval_expr("0xDEAD", &[]), 0xDEAD as f64);
        assert_eq!(eval_expr("(0x01080001 >> 16) & 0xFF", &[]), 8.0);
        // The aravis PayloadSize formula
        assert_eq!(
            eval_expr(
                "W * H * ((PF>>16)&0xFF) / 8",
                &[("W", 512.0), ("H", 512.0), ("PF", 0x01080001_u32 as f64)]
            ),
            512.0 * 512.0 * 8.0 / 8.0
        );
    }

    #[test]
    fn power_operator() {
        assert!((eval_expr("2 ** 3", &[]) - 8.0).abs() < 1e-6);
        assert!((eval_expr("2 ** 3 ** 2", &[]) - 512.0).abs() < 1e-6); // Right-associative: 2^(3^2) = 2^9
    }

    #[test]
    fn modulo_operator() {
        assert!((eval_expr("10 % 3", &[]) - 1.0).abs() < 1e-6);
        assert!((eval_expr("17 % 5", &[]) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn functions() {
        assert!((eval_expr("abs(-5)", &[]) - 5.0).abs() < 1e-6);
        assert!((eval_expr("sqrt(16)", &[]) - 4.0).abs() < 1e-6);
        assert!((eval_expr("min(3, 7)", &[]) - 3.0).abs() < 1e-6);
        assert!((eval_expr("max(3, 7)", &[]) - 7.0).abs() < 1e-6);
        assert!((eval_expr("pow(2, 10)", &[]) - 1024.0).abs() < 1e-6);
        assert!((eval_expr("floor(3.7)", &[]) - 3.0).abs() < 1e-6);
        assert!((eval_expr("ceil(3.2)", &[]) - 4.0).abs() < 1e-6);
        assert!((eval_expr("round(3.5)", &[]) - 4.0).abs() < 1e-6);
        assert!((eval_expr("sgn(-5)", &[]) - -1.0).abs() < 1e-6);
        assert!((eval_expr("sgn(5)", &[]) - 1.0).abs() < 1e-6);
        assert!((eval_expr("sgn(0)", &[]) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn scientific_notation() {
        assert!((eval_expr("1e3", &[]) - 1000.0).abs() < 1e-6);
        assert!((eval_expr("1.5e-2", &[]) - 0.015).abs() < 1e-9);
        assert!((eval_expr("2.5E+3", &[]) - 2500.0).abs() < 1e-6);
    }

    #[test]
    fn division_by_zero_error() {
        let ast = parse_expression("A / B").expect("parse");
        let mut vars = |name: &str| match name {
            "A" => Ok(5.0),
            "B" => Ok(0.0),
            _ => Err(EvalError::UnknownVariable(name.to_string())),
        };
        let err = evaluate(&ast, &mut vars).expect_err("division by zero");
        assert!(matches!(err, EvalError::DivisionByZero));
    }

    #[test]
    fn complex_basler_style_expression() {
        // Basler cameras often use expressions like this for exposure time conversion
        let expr = "RawValue < 0 ? 0 : RawValue * 1000 / TickFreq";
        assert!(
            (eval_expr(expr, &[("RawValue", 500.0), ("TickFreq", 1000.0)]) - 500.0).abs() < 1e-6
        );
        assert_eq!(
            eval_expr(expr, &[("RawValue", -10.0), ("TickFreq", 1000.0)]),
            0.0
        );
    }

    #[test]
    fn collect_identifiers_with_ternary() {
        let ast = parse_expression("A > B ? C + D : E * F").expect("parse");
        let mut ids = HashSet::new();
        collect_identifiers(&ast, &mut ids);
        assert!(ids.contains("A"));
        assert!(ids.contains("B"));
        assert!(ids.contains("C"));
        assert!(ids.contains("D"));
        assert!(ids.contains("E"));
        assert!(ids.contains("F"));
        assert_eq!(ids.len(), 6);
    }

    #[test]
    fn collect_identifiers_with_functions() {
        let ast = parse_expression("max(A, min(B, C))").expect("parse");
        let mut ids = HashSet::new();
        collect_identifiers(&ast, &mut ids);
        assert!(ids.contains("A"));
        assert!(ids.contains("B"));
        assert!(ids.contains("C"));
        assert_eq!(ids.len(), 3);
    }
}
