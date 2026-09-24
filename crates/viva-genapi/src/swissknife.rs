//! GenApi formula parser and evaluator.
//!
//! Covers the expression language used by `<SwissKnife>`, `<IntSwissKnife>`,
//! `<Converter>` and `<IntConverter>` nodes:
//!
//! - Arithmetic: `+ - * / %` and `**` for power
//! - Comparison: `< <= > >=`, `=` (equality) and `<>` (inequality)
//! - Logical: `&& ||`
//! - Bitwise: `& | ^ ~ << >>`
//! - Ternary: `condition ? then : else`
//! - Functions: `SGN NEG ATAN SIN COS TAN ABS EXP LN LG SQRT TRUNC ROUND
//!   FLOOR CEIL ASIN ACOS`, plus the constants `E` and `PI`
//!
//! Two details trip up implementations that reach for a C-like grammar, and
//! both appear in the majority of real vendor descriptions:
//!
//! 1. **`=` is equality, `<>` is inequality.** There is no assignment in the
//!    language, so `=` is never ambiguous. `==` and `!=` are accepted as
//!    tolerated aliases.
//! 2. **Integer formulas evaluate in `i64`, not `f64`.** `IntSwissKnife` and
//!    `IntConverter` use [`EvalMode::Integer`], where `/` truncates and 64-bit
//!    register values stay exact. Evaluating `(HIGH << 32) | LOW` in `f64`
//!    silently loses the low bits.
//!
//! Operator precedence and the integer/float promotion rules follow the GenApi
//! specification, cross-checked against the reference implementation in
//! aravis (`src/arvevaluator.c`: `arv_evaluator_token_infos` for precedence,
//! the `integer_mode` branches of `arv_evaluator_evaluate` for promotion).

use std::collections::HashSet;
use std::fmt;

/// Numeric value flowing through a GenApi formula.
///
/// The language is dynamically typed over `i64` and `f64`. Keeping the two
/// apart matters: register values are integers up to 64 bits wide, and `f64`
/// cannot represent them all exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    /// Integer value.
    Int(i64),
    /// Floating point value.
    Float(f64),
}

impl Value {
    /// View the value as an `i64`, truncating a float towards zero.
    pub fn as_i64(self) -> i64 {
        match self {
            Value::Int(value) => value,
            Value::Float(value) => value as i64,
        }
    }

    /// View the value as an `f64`.
    pub fn as_f64(self) -> f64 {
        match self {
            Value::Int(value) => value as f64,
            Value::Float(value) => value,
        }
    }

    /// Whether the value is held as an integer.
    pub fn is_int(self) -> bool {
        matches!(self, Value::Int(_))
    }

    /// Whether the value counts as true in a condition (non-zero).
    pub fn is_truthy(self) -> bool {
        match self {
            Value::Int(value) => value != 0,
            Value::Float(value) => value != 0.0,
        }
    }

    fn from_bool(value: bool) -> Self {
        Value::Int(i64::from(value))
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(value) => write!(f, "{value}"),
            Value::Float(value) => write!(f, "{value}"),
        }
    }
}

/// Arithmetic mode for a formula.
///
/// `IntSwissKnife` and `IntConverter` declare integer semantics; `SwissKnife`
/// and `Converter` declare floating point ones. The distinction is observable:
/// `7 / 2` is `3` in [`EvalMode::Integer`] and `3.5` in [`EvalMode::Float`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EvalMode {
    /// Integer arithmetic: `/` truncates, results stay in `i64`.
    Integer,
    /// Floating point arithmetic. Bitwise operators still work on `i64`.
    #[default]
    Float,
}

/// Parsed GenApi formula represented as an abstract syntax tree.
#[derive(Debug, Clone)]
pub enum AstNode {
    /// Numeric literal.
    Literal(Value),
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

/// Binary operator kinds supported by the GenApi formula language.
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

/// Unary operator kinds supported by the GenApi formula language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Plus,
    Minus,
    Not,
    BitNot,
}

/// Error produced while parsing a GenApi formula.
#[derive(Debug, Clone)]
pub struct ParseError {
    msg: String,
    offset: usize,
}

impl ParseError {
    fn new<S: Into<String>>(msg: S, offset: usize) -> Self {
        Self {
            msg: msg.into(),
            offset,
        }
    }

    /// Byte offset within the formula at which the error was detected.
    pub fn offset(&self) -> usize {
        self.offset
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at offset {})", self.msg, self.offset)
    }
}

impl std::error::Error for ParseError {}

/// Error produced while evaluating a GenApi formula.
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

/// Parse a GenApi formula into an [`AstNode`].
pub fn parse_expression(input: &str) -> Result<AstNode, ParseError> {
    let mut parser = Parser::new(input)?;
    let expr = parser.parse_ternary()?;
    if !matches!(parser.lookahead, Token::End) {
        return Err(ParseError::new("unexpected trailing tokens", parser.pos));
    }
    Ok(expr)
}

/// Value of a GenApi built-in constant, if `name` denotes one.
///
/// The language defines `E` and `PI` as constants rather than variables, so a
/// formula may reference them without a matching `<pVariable>`. Matching is
/// case-insensitive, as in the reference implementation.
pub fn builtin_constant(name: &str) -> Option<Value> {
    match name.to_ascii_lowercase().as_str() {
        "e" => Some(Value::Float(std::f64::consts::E)),
        "pi" => Some(Value::Float(std::f64::consts::PI)),
        _ => None,
    }
}

/// Whether `name` denotes a GenApi built-in constant.
///
/// Callers validating that every identifier in a formula has a `<pVariable>`
/// must exempt these.
pub fn is_builtin_constant(name: &str) -> bool {
    builtin_constant(name).is_some()
}

/// The property of a `<pVariable>`'s node that a formula identifier reads.
///
/// GenICam Standard v2.1.1, §2.8.13 (SwissKnife, IntSwissKnife, Converter,
/// and IntConverter): "you can access the minimum, maximum, and increment of a
/// node by using the variable name extensions .Min, .Max, .Inc, and – for
/// completeness – also .Value. In addition .Entry.Name is allowed which
/// accesses the integer value of an EnumEntry described by Name."
///
/// The standard's own example puts the extension in the declaration —
/// `<pVariable Name="Gain.Max">Gain</pVariable>` — and a formula may also
/// qualify a plainly declared variable (`V1.Min` with only `V1` declared).
/// Both resolve to the same thing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum VarAttr {
    /// The node's value — what a plain identifier reads.
    Value,
    /// The node's minimum (`<pMin>` or `<Min>`).
    Min,
    /// The node's maximum (`<pMax>` or `<Max>`).
    Max,
    /// The node's increment (`<Inc>`, 1 when undeclared).
    Inc,
    /// The integer value of the enumeration entry with this `Name`.
    Entry(String),
}

impl VarAttr {
    /// Parse the extension after a variable name's first `.`: `Value`, `Min`,
    /// `Max`, `Inc` or `Entry.<EntryName>`.
    ///
    /// The keyword is matched case-insensitively, since formula variables are
    /// conventionally upper case; the entry name is matched exactly. Returns
    /// `None` for anything else.
    pub fn parse(extension: &str) -> Option<Self> {
        let (keyword, rest) = match extension.split_once('.') {
            Some((keyword, rest)) => (keyword, Some(rest)),
            None => (extension, None),
        };
        let attr = match (keyword.to_ascii_lowercase().as_str(), rest) {
            ("value", None) => VarAttr::Value,
            ("min", None) => VarAttr::Min,
            ("max", None) => VarAttr::Max,
            ("inc", None) => VarAttr::Inc,
            ("entry", Some(entry)) if !entry.is_empty() && !entry.contains('.') => {
                VarAttr::Entry(entry.to_string())
            }
            _ => return None,
        };
        Some(attr)
    }
}

/// Split a qualified identifier such as `V1.Min` at its first `.`.
///
/// Returns `None` for a plain identifier. The lexer admits `.` inside an
/// identifier (a number such as `1.5` never reaches here, because it starts
/// with a digit), so this is where a variable is told apart from its
/// extension.
pub fn split_qualified(ident: &str) -> Option<(&str, &str)> {
    ident.split_once('.')
}

/// Evaluate an [`AstNode`] using the provided variable resolver.
///
/// The resolver receives variable identifiers and must return their value.
/// An identifier the resolver rejects with [`EvalError::UnknownVariable`] is
/// retried against the built-in constants before the error is propagated, so a
/// declared `<pVariable>` always shadows a same-named constant.
pub fn evaluate(
    ast: &AstNode,
    vars: &mut dyn FnMut(&str) -> Result<Value, EvalError>,
    mode: EvalMode,
) -> Result<Value, EvalError> {
    match ast {
        AstNode::Literal(value) => Ok(*value),
        AstNode::Variable(name) => match vars(name) {
            Ok(value) => Ok(value),
            Err(EvalError::UnknownVariable(_)) => {
                builtin_constant(name).ok_or_else(|| EvalError::UnknownVariable(name.clone()))
            }
            Err(other) => Err(other),
        },
        AstNode::Unary { op, expr } => {
            let inner = evaluate(expr, vars, mode)?;
            Ok(eval_unary(*op, inner, mode))
        }
        AstNode::Binary { op, left, right } => {
            // Short-circuit evaluation for logical operators
            match op {
                BinaryOp::And => {
                    let lhs = evaluate(left, vars, mode)?;
                    if !lhs.is_truthy() {
                        return Ok(Value::from_bool(false));
                    }
                    let rhs = evaluate(right, vars, mode)?;
                    Ok(Value::from_bool(rhs.is_truthy()))
                }
                BinaryOp::Or => {
                    let lhs = evaluate(left, vars, mode)?;
                    if lhs.is_truthy() {
                        return Ok(Value::from_bool(true));
                    }
                    let rhs = evaluate(right, vars, mode)?;
                    Ok(Value::from_bool(rhs.is_truthy()))
                }
                _ => {
                    let lhs = evaluate(left, vars, mode)?;
                    let rhs = evaluate(right, vars, mode)?;
                    eval_binary(*op, lhs, rhs, mode)
                }
            }
        }
        AstNode::Ternary {
            cond,
            then_expr,
            else_expr,
        } => {
            if evaluate(cond, vars, mode)?.is_truthy() {
                evaluate(then_expr, vars, mode)
            } else {
                evaluate(else_expr, vars, mode)
            }
        }
        AstNode::FnCall { name, args } => {
            let evaluated: Result<Vec<Value>, _> =
                args.iter().map(|a| evaluate(a, vars, mode)).collect();
            let arg_vals = evaluated?;
            eval_function(name, &arg_vals, mode)
        }
    }
}

fn eval_unary(op: UnaryOp, value: Value, mode: EvalMode) -> Value {
    match op {
        UnaryOp::Plus => value,
        UnaryOp::Minus => {
            if mode == EvalMode::Integer || value.is_int() {
                Value::Int(value.as_i64().wrapping_neg())
            } else {
                Value::Float(-value.as_f64())
            }
        }
        UnaryOp::Not => Value::from_bool(!value.is_truthy()),
        UnaryOp::BitNot => Value::Int(!value.as_i64()),
    }
}

/// Whether an arithmetic result should stay integral.
fn integral(mode: EvalMode, lhs: Value, rhs: Value) -> bool {
    mode == EvalMode::Integer || (lhs.is_int() && rhs.is_int())
}

/// Shift counts are masked to the register width, matching the reference
/// implementation's reliance on hardware shift semantics. Without this,
/// `HIGH << 32` on a wide value panics in a debug build.
fn shift_amount(value: Value) -> u32 {
    (value.as_i64() as u64 & 63) as u32
}

fn eval_binary(op: BinaryOp, lhs: Value, rhs: Value, mode: EvalMode) -> Result<Value, EvalError> {
    Ok(match op {
        BinaryOp::Add => {
            if integral(mode, lhs, rhs) {
                Value::Int(lhs.as_i64().wrapping_add(rhs.as_i64()))
            } else {
                Value::Float(lhs.as_f64() + rhs.as_f64())
            }
        }
        BinaryOp::Sub => {
            if integral(mode, lhs, rhs) {
                Value::Int(lhs.as_i64().wrapping_sub(rhs.as_i64()))
            } else {
                Value::Float(lhs.as_f64() - rhs.as_f64())
            }
        }
        BinaryOp::Mul => {
            if integral(mode, lhs, rhs) {
                Value::Int(lhs.as_i64().wrapping_mul(rhs.as_i64()))
            } else {
                Value::Float(lhs.as_f64() * rhs.as_f64())
            }
        }
        // Division is the one operator where the mode alone decides: an
        // integer formula truncates even when a literal happens to be
        // fractional, and a float formula never truncates.
        BinaryOp::Div => {
            if mode == EvalMode::Integer {
                let divisor = rhs.as_i64();
                if divisor == 0 {
                    return Err(EvalError::DivisionByZero);
                }
                Value::Int(lhs.as_i64().wrapping_div(divisor))
            } else {
                let divisor = rhs.as_f64();
                if divisor == 0.0 {
                    return Err(EvalError::DivisionByZero);
                }
                Value::Float(lhs.as_f64() / divisor)
            }
        }
        BinaryOp::Mod => {
            let divisor = rhs.as_i64();
            if divisor == 0 {
                return Err(EvalError::DivisionByZero);
            }
            Value::Int(lhs.as_i64().wrapping_rem(divisor))
        }
        BinaryOp::Pow => {
            let result = lhs.as_f64().powf(rhs.as_f64());
            if mode == EvalMode::Integer {
                Value::Int(result as i64)
            } else {
                Value::Float(result)
            }
        }
        BinaryOp::Lt => compare(lhs, rhs, mode, |a, b| a < b, |a, b| a < b),
        BinaryOp::Le => compare(lhs, rhs, mode, |a, b| a <= b, |a, b| a <= b),
        BinaryOp::Gt => compare(lhs, rhs, mode, |a, b| a > b, |a, b| a > b),
        BinaryOp::Ge => compare(lhs, rhs, mode, |a, b| a >= b, |a, b| a >= b),
        BinaryOp::Eq => compare(lhs, rhs, mode, |a, b| a == b, |a, b| a == b),
        BinaryOp::Ne => compare(lhs, rhs, mode, |a, b| a != b, |a, b| a != b),
        BinaryOp::And | BinaryOp::Or => unreachable!("handled by short-circuit"),
        BinaryOp::BitAnd => Value::Int(lhs.as_i64() & rhs.as_i64()),
        BinaryOp::BitOr => Value::Int(lhs.as_i64() | rhs.as_i64()),
        BinaryOp::BitXor => Value::Int(lhs.as_i64() ^ rhs.as_i64()),
        BinaryOp::Shl => Value::Int(lhs.as_i64().wrapping_shl(shift_amount(rhs))),
        BinaryOp::Shr => Value::Int(lhs.as_i64().wrapping_shr(shift_amount(rhs))),
    })
}

/// Compare two values, exactly when both sides are integral.
///
/// Comparing register values through `f64` would make `0x100000000000001` and
/// `0x100000000000000` equal; comparing them as `i64` does not.
fn compare(
    lhs: Value,
    rhs: Value,
    mode: EvalMode,
    int_cmp: fn(i64, i64) -> bool,
    float_cmp: fn(f64, f64) -> bool,
) -> Value {
    if integral(mode, lhs, rhs) {
        Value::from_bool(int_cmp(lhs.as_i64(), rhs.as_i64()))
    } else {
        Value::from_bool(float_cmp(lhs.as_f64(), rhs.as_f64()))
    }
}

fn eval_function(name: &str, args: &[Value], mode: EvalMode) -> Result<Value, EvalError> {
    // GenApi function names are case-insensitive; vendors write both `LG` and
    // `lg`, `SGN` and `sgn`.
    let name_lower = name.to_ascii_lowercase();

    // Transcendental functions always produce a float, matching the reference
    // implementation.
    let float_fn = |f: fn(f64) -> f64| -> Result<Value, EvalError> {
        expect_args(name, args, 1).map(|a| Value::Float(f(a[0].as_f64())))
    };
    // Functions that preserve integrality when their argument is integral.
    let rounding_fn = |f: fn(f64) -> f64| -> Result<Value, EvalError> {
        expect_args(name, args, 1).map(|a| {
            if mode == EvalMode::Integer || a[0].is_int() {
                Value::Int(a[0].as_i64())
            } else {
                Value::Float(f(a[0].as_f64()))
            }
        })
    };

    match name_lower.as_str() {
        // --- GenApi standard functions -----------------------------------
        "sin" => float_fn(f64::sin),
        "cos" => float_fn(f64::cos),
        "tan" => float_fn(f64::tan),
        "asin" => float_fn(f64::asin),
        "acos" => float_fn(f64::acos),
        "atan" => float_fn(f64::atan),
        "sqrt" => float_fn(f64::sqrt),
        "exp" => float_fn(f64::exp),
        "ln" => float_fn(f64::ln),
        // `LG` is the base-10 logarithm. Omitting it made every Baumer TXG
        // description fail at evaluation time.
        "lg" => float_fn(f64::log10),
        "trunc" => rounding_fn(f64::trunc),
        "floor" => rounding_fn(f64::floor),
        "ceil" => rounding_fn(f64::ceil),
        "round" => rounding_fn(f64::round),
        "abs" => expect_args(name, args, 1).map(|a| {
            if mode == EvalMode::Integer || a[0].is_int() {
                Value::Int(a[0].as_i64().wrapping_abs())
            } else {
                Value::Float(a[0].as_f64().abs())
            }
        }),
        "neg" => expect_args(name, args, 1).map(|a| eval_unary(UnaryOp::Minus, a[0], mode)),
        "sgn" | "sign" => expect_args(name, args, 1).map(|a| {
            if mode == EvalMode::Integer || a[0].is_int() {
                Value::Int(a[0].as_i64().signum())
            } else {
                let value = a[0].as_f64();
                Value::Float(if value > 0.0 {
                    1.0
                } else if value < 0.0 {
                    -1.0
                } else {
                    0.0
                })
            }
        }),
        // The constants are usually written bare, but the call form appears
        // in the wild too.
        "e" => expect_args(name, args, 0).map(|_| Value::Float(std::f64::consts::E)),
        "pi" => expect_args(name, args, 0).map(|_| Value::Float(std::f64::consts::PI)),

        // --- Accepted extensions ------------------------------------------
        // Not in the GenApi standard, but harmless to accept and already
        // relied on by our own fixtures.
        "log" | "log10" => float_fn(f64::log10),
        "log2" => float_fn(f64::log2),
        "atan2" => {
            expect_args(name, args, 2).map(|a| Value::Float(a[0].as_f64().atan2(a[1].as_f64())))
        }
        "pow" => eval_binary(BinaryOp::Pow, args_at(name, args, 2)?[0], args[1], mode),
        "fmod" => eval_binary(BinaryOp::Mod, args_at(name, args, 2)?[0], args[1], mode),
        "min" => expect_args(name, args, 2).map(|a| pick(a[0], a[1], mode, true)),
        "max" => expect_args(name, args, 2).map(|a| pick(a[0], a[1], mode, false)),

        _ => Err(EvalError::UnknownFunction(name.to_string())),
    }
}

fn pick(lhs: Value, rhs: Value, mode: EvalMode, want_min: bool) -> Value {
    let take_lhs = if integral(mode, lhs, rhs) {
        (lhs.as_i64() <= rhs.as_i64()) == want_min
    } else {
        (lhs.as_f64() <= rhs.as_f64()) == want_min
    };
    if take_lhs { lhs } else { rhs }
}

fn args_at<'a>(name: &str, args: &'a [Value], expected: usize) -> Result<&'a [Value], EvalError> {
    expect_args(name, args, expected)
}

fn expect_args<'a>(
    name: &str,
    args: &'a [Value],
    expected: usize,
) -> Result<&'a [Value], EvalError> {
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

/// Replace variable references with sub-expressions.
///
/// Backs the GenApi `<Constant>` and named `<Expression>` elements, which let
/// a formula name a literal or a reusable sub-formula:
///
/// ```xml
/// <IntSwissKnife Name="Example">
///   <pVariable Name="X">X</pVariable>
///   <Constant Name="TEN">10</Constant>
///   <Expression Name="XPLUS2">TEN + X</Expression>
///   <Formula>TEN * XPLUS2</Formula>
/// </IntSwissKnife>
/// ```
///
/// Substitution happens once, at build time, so evaluation stays a plain walk
/// over the tree.
pub fn substitute(ast: &mut AstNode, bindings: &std::collections::HashMap<String, AstNode>) {
    match ast {
        AstNode::Literal(_) => {}
        AstNode::Variable(name) => {
            if let Some(replacement) = bindings.get(name.as_str()) {
                *ast = replacement.clone();
            }
        }
        AstNode::Unary { expr, .. } => substitute(expr, bindings),
        AstNode::Binary { left, right, .. } => {
            substitute(left, bindings);
            substitute(right, bindings);
        }
        AstNode::Ternary {
            cond,
            then_expr,
            else_expr,
        } => {
            substitute(cond, bindings);
            substitute(then_expr, bindings);
            substitute(else_expr, bindings);
        }
        AstNode::FnCall { args, .. } => {
            for arg in args {
                substitute(arg, bindings);
            }
        }
    }
}

/// Collect all variable identifiers referenced by the AST.
///
/// Built-in constants are reported like any other identifier; callers that
/// validate against `<pVariable>` declarations should filter them with
/// [`is_builtin_constant`].
pub fn collect_identifiers(ast: &AstNode, out: &mut HashSet<String>) {
    match ast {
        AstNode::Literal(_) => {}
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
    Int(i64),
    Float(f64),
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
    Eq, // `=` (and the tolerated `==`)
    Ne, // `<>` (and the tolerated `!=`)
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
                // `<>` is the GenApi inequality operator.
                Some(b'>') => {
                    self.advance_by(2);
                    Ok(Token::Ne)
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
            // `=` is equality. The language has no assignment, so there is
            // nothing to disambiguate; `==` is accepted as an alias.
            b'=' => {
                if self.peek_next() == Some(b'=') {
                    self.advance_by(2);
                } else {
                    self.advance_by(1);
                }
                Ok(Token::Eq)
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
            _ => Err(ParseError::new(
                format!("unexpected character '{}'", byte as char),
                self.pos,
            )),
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
                    return Err(ParseError::new("hex literal has no digits", start));
                }
                let hex_text = std::str::from_utf8(&self.input[hex_start..self.pos])
                    .map_err(|_| ParseError::new("invalid UTF-8 in hex literal", start))?;
                let value = u64::from_str_radix(hex_text, 16).map_err(|_| {
                    ParseError::new(format!("invalid hex literal: 0x{hex_text}"), start)
                })?;
                // Masks such as 0xFFFFFFFFFFFFFFFF wrap to their two's
                // complement, as they do in the reference implementation.
                return Ok(Token::Int(value as i64));
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
                    // `1e3` is scientific notation, but `1 e` would be a
                    // literal followed by the constant E. Only treat it as an
                    // exponent when a digit or sign follows.
                    match self.peek_next() {
                        Some(b'0'..=b'9' | b'+' | b'-') => {
                            seen_exp = true;
                            self.pos += 1;
                            if let Some(b'+' | b'-') = self.peek() {
                                self.pos += 1;
                            }
                        }
                        _ => break,
                    }
                }
                _ => break,
            }
        }
        if !seen_digit {
            return Err(ParseError::new("invalid number literal", start));
        }
        let slice = &self.input[start..self.pos];
        let text = std::str::from_utf8(slice)
            .map_err(|_| ParseError::new("invalid UTF-8 in number", start))?;
        if !seen_dot
            && !seen_exp
            && let Ok(value) = text.parse::<i64>()
        {
            return Ok(Token::Int(value));
        }
        let value = text
            .parse::<f64>()
            .map_err(|_| ParseError::new(format!("failed to parse number: {text}"), start))?;
        Ok(Token::Float(value))
    }

    fn lex_ident(&mut self) -> Result<Token, ParseError> {
        let start = self.pos;
        self.pos += 1;
        while let Some(byte) = self.peek() {
            if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let slice = &self.input[start..self.pos];
        let text = std::str::from_utf8(slice)
            .map_err(|_| ParseError::new("invalid UTF-8 in identifier", start))?;
        Ok(Token::Ident(text.to_string()))
    }
}

// ============================================================================
// Parser
//
// Precedence, loosest to tightest. This chain mirrors the priority column of
// aravis' `arv_evaluator_token_infos` table, which is itself the GenApi
// specification's table:
//
//  1. Ternary:        ?:      (right-associative)
//  2. Logical OR:     ||
//  3. Logical AND:    &&
//  4. Bitwise OR:     |
//  5. Bitwise XOR:    ^
//  6. Bitwise AND:    &
//  7. Equality:       =  <>
//  8. Comparison:     <  <=  >  >=
//  9. Shift:          << >>
// 10. Additive:       +  -
// 11. Multiplicative: *  /  %
// 12. Power:          **      (right-associative)
// 13. Unary:          +  -  !  ~
// 14. Primary:        numbers, identifiers, function calls, (expr)
//
// Note that equality binds *looser* than comparison, so `A < B = C` parses as
// `(A < B) = C`. That is what the standard says, and vendors rely on it.
// ============================================================================

struct Parser<'a> {
    lexer: Lexer<'a>,
    lookahead: Token,
    /// Offset of the start of the lookahead token, for error reporting.
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Result<Self, ParseError> {
        let mut lexer = Lexer::new(input);
        let lookahead = lexer.next_token()?;
        let pos = lexer.pos;
        Ok(Parser {
            lexer,
            lookahead,
            pos,
        })
    }

    fn advance(&mut self) -> Result<(), ParseError> {
        self.lookahead = self.lexer.next_token()?;
        self.pos = self.lexer.pos;
        Ok(())
    }

    // Level 1: Ternary
    fn parse_ternary(&mut self) -> Result<AstNode, ParseError> {
        let cond = self.parse_or()?;
        if matches!(self.lookahead, Token::Question) {
            self.advance()?;
            let then_expr = self.parse_ternary()?;
            if !matches!(self.lookahead, Token::Colon) {
                return Err(ParseError::new(
                    "expected ':' in ternary expression",
                    self.pos,
                ));
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
                Token::Eq => BinaryOp::Eq,
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
        let op = match &self.lookahead {
            Token::Plus => UnaryOp::Plus,
            Token::Minus => UnaryOp::Minus,
            Token::Bang => UnaryOp::Not,
            Token::Tilde => UnaryOp::BitNot,
            _ => return self.parse_primary(),
        };
        self.advance()?;
        let expr = self.parse_unary()?;
        Ok(AstNode::Unary {
            op,
            expr: Box::new(expr),
        })
    }

    // Level 14: Primary
    fn parse_primary(&mut self) -> Result<AstNode, ParseError> {
        match self.lookahead.clone() {
            Token::Int(value) => {
                self.advance()?;
                Ok(AstNode::Literal(Value::Int(value)))
            }
            Token::Float(value) => {
                self.advance()?;
                Ok(AstNode::Literal(Value::Float(value)))
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
                        return Err(ParseError::new(
                            "expected ')' after function arguments",
                            self.pos,
                        ));
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
                    return Err(ParseError::new("missing closing ')'", self.pos));
                }
                self.advance()?;
                Ok(expr)
            }
            Token::End => Err(ParseError::new("unexpected end of expression", self.pos)),
            other => Err(ParseError::new(
                format!("unexpected token {other:?}"),
                self.pos,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval_with(expr: &str, vars: &[(&str, Value)], mode: EvalMode) -> Value {
        let ast = parse_expression(expr).expect("parse failed");
        let mut resolver = |name: &str| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| *v)
                .ok_or_else(|| EvalError::UnknownVariable(name.to_string()))
        };
        evaluate(&ast, &mut resolver, mode).expect("eval failed")
    }

    /// Evaluate in float mode with float variables (the `SwissKnife` case).
    fn eval_expr(expr: &str, vars: &[(&str, f64)]) -> f64 {
        let bound: Vec<(&str, Value)> = vars.iter().map(|(n, v)| (*n, Value::Float(*v))).collect();
        eval_with(expr, &bound, EvalMode::Float).as_f64()
    }

    /// Evaluate in integer mode with integer variables (the `IntSwissKnife`
    /// case).
    fn eval_int(expr: &str, vars: &[(&str, i64)]) -> i64 {
        let bound: Vec<(&str, Value)> = vars.iter().map(|(n, v)| (*n, Value::Int(*v))).collect();
        eval_with(expr, &bound, EvalMode::Integer).as_i64()
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
        assert_eq!(eval_expr("A < B", &[("A", 3.0), ("B", 5.0)]), 1.0);
    }

    /// `=` is the GenApi equality operator. Rejecting it made 27 of the 30
    /// documents in the vendor corpus unopenable (issue #35).
    #[test]
    fn single_equals_is_equality() {
        assert_eq!(eval_int("5 = 5", &[]), 1);
        assert_eq!(eval_int("5 = 6", &[]), 0);
        // The formula from the reporter's Hikrobot MV-CS050-10GC.
        assert_eq!(
            eval_int("(TEMPCTRLMODE = 1) ? 1 : 0", &[("TEMPCTRLMODE", 1)]),
            1
        );
        assert_eq!(
            eval_int("(TEMPCTRLMODE = 1) ? 1 : 0", &[("TEMPCTRLMODE", 0)]),
            0
        );
        // `==` stays accepted.
        assert_eq!(eval_int("5 == 5", &[]), 1);
    }

    /// `<>` is the GenApi inequality operator.
    #[test]
    fn angle_brackets_are_inequality() {
        assert_eq!(eval_int("5 <> 6", &[]), 1);
        assert_eq!(eval_int("5 <> 5", &[]), 0);
        assert_eq!(eval_int("(DS <> 0) ? 1 : 0", &[("DS", 3)]), 1);
        // Still distinct from shift and comparison.
        assert_eq!(eval_int("1 << 3", &[]), 8);
        assert_eq!(eval_int("(1 < 3) = 1", &[]), 1);
        // `!=` stays accepted.
        assert_eq!(eval_int("5 != 5", &[]), 0);
    }

    #[test]
    fn equality_binds_looser_than_comparison() {
        // `A < B = C` is `(A < B) = C`, not `A < (B = C)`.
        assert_eq!(eval_int("1 < 3 = 1", &[]), 1);
        assert_eq!(eval_int("3 < 1 = 0", &[]), 1);
        // And bitwise AND binds looser than equality.
        assert_eq!(eval_int("1 & 1 = 1", &[]), 1);
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
        assert_eq!(eval_int("~0", &[]), -1);
    }

    /// A 64-bit register split across two 32-bit halves is the canonical
    /// reason integer formulas cannot go through `f64`: the low bits of the
    /// result are not representable.
    #[test]
    fn wide_shifts_stay_exact() {
        assert_eq!(
            eval_int(
                "(HIGH << 32) | LOW",
                &[("HIGH", 0x1234_5678), ("LOW", 0x9ABC_DEF1)]
            ),
            0x1234_5678_9ABC_DEF1
        );
        // MAC address composition, as used by GevMACAddrHigh/Low.
        assert_eq!(
            eval_int(
                "( ( HI & 0x0000FFFF ) << 32 ) | LO",
                &[("HI", 0x205C), ("LO", 0x208F_8000)]
            ),
            0x205C_208F_8000
        );
        // A shift wide enough to overflow must not panic in a debug build.
        assert_eq!(eval_int("V << 63", &[("V", 3)]), i64::MIN);
        assert_eq!(eval_int("V << 64", &[("V", 3)]), 3);
    }

    /// Integer formulas truncate on division; float formulas do not.
    #[test]
    fn division_follows_the_mode() {
        assert_eq!(eval_int("7 / 2", &[]), 3);
        assert_eq!(eval_int("(IDX / 2) * 4", &[("IDX", 3)]), 4);
        assert!((eval_expr("7 / 2", &[]) - 3.5).abs() < 1e-9);
        // Reported by the Hikrobot: an offset table that only lands on the
        // right register because the division truncates.
        assert_eq!(eval_int("OFFSET * 4 / 2", &[("OFFSET", 3)]), 6);
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
        // Lowercase hex digits, as Hikrobot writes them.
        assert_eq!(eval_int("PF = 0x0110000c", &[("PF", 0x0110000C)]), 1);
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

    /// `LG` is base-10 log. Baumer TXG descriptions use it for a dB scale.
    #[test]
    fn genapi_standard_functions() {
        assert!((eval_expr("LG(1000)", &[]) - 3.0).abs() < 1e-9);
        assert!((eval_expr("(20 * (LG(TO/1024)))", &[("TO", 10240.0)]) - 20.0).abs() < 1e-9);
        assert!((eval_expr("LN(1)", &[]) - 0.0).abs() < 1e-9);
        assert!((eval_expr("TRUNC(3.9)", &[]) - 3.0).abs() < 1e-9);
        assert!((eval_expr("NEG(4)", &[]) + 4.0).abs() < 1e-9);
        // Names are case-insensitive.
        assert!((eval_expr("Sqrt(9)", &[]) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn builtin_constants() {
        assert!((eval_expr("PI", &[]) - std::f64::consts::PI).abs() < 1e-12);
        assert!((eval_expr("E", &[]) - std::f64::consts::E).abs() < 1e-12);
        assert!((eval_expr("2 * PI", &[]) - std::f64::consts::TAU).abs() < 1e-12);
        // A declared variable shadows the constant.
        assert_eq!(eval_expr("E", &[("E", 7.0)]), 7.0);
        assert!(is_builtin_constant("pi"));
        assert!(!is_builtin_constant("EXPMODE"));
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
            "A" => Ok(Value::Float(5.0)),
            "B" => Ok(Value::Float(0.0)),
            _ => Err(EvalError::UnknownVariable(name.to_string())),
        };
        let err = evaluate(&ast, &mut vars, EvalMode::Float).expect_err("division by zero");
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

    /// A representative selection of the shapes seen across the vendor corpus
    /// and in the reporter's Hikrobot dump: multi-line formulas, chained
    /// ternaries, `=`/`<>` mixed with bitwise masks.
    #[test]
    fn vendor_formula_shapes_parse() {
        let formulas = [
            "( ( PINPRES = 1 ) && ( MODE = 0 ) && (SEL <> 2) ) ? 1 : 0",
            "((CTRL_REG & 0x03000000)=0x03000000)?1:0",
            "((CTRL_REG | 0xFDFFFFFF)=0xFFFFFFFF)?0:1",
            "ADDROFFSET = 0 ? 0 : ADDROFFSET * 0x80",
            "(FEAT<32)?((GEVOPT>>FEAT)&0x1):((FEAT<64)?((IPOPT>>(FEAT-32))&0x1) : ((FEAT=66)?SCCB:0))",
            "(   ((TM = 0) && (LINE1 = 1)) \n || ((TL = 1) && (REVERSE = 0)) \n || (DS <> 0)) ? 1 : 0",
            "((TYPE = 0x011a) || (TYPE = 0x011b)) ? (0.4232 * DKELVIN - 334.83) : (DKELVIN /100)",
            "(SHUTTERMODE = 0) && (ROLLING = 1)",
            "INDEX = 0",
            "( MAX % UNIT ) ? ( MAX - (  MAX) % UNIT  ) : ( MAX)",
            "V1.Min",
            "V1.Max",
        ];
        for formula in formulas {
            parse_expression(formula)
                .unwrap_or_else(|err| panic!("failed to parse {formula:?}: {err}"));
        }
    }

    /// GA-29: `.` is admitted into identifiers for `V1.Min`, and must not take
    /// number literals with it.
    #[test]
    fn qualified_identifiers_and_decimal_literals_lex_apart() {
        let ast = parse_expression("V1.Max - 1.5 + .5").expect("parse");
        let mut ids = HashSet::new();
        collect_identifiers(&ast, &mut ids);
        assert_eq!(ids, HashSet::from(["V1.Max".to_string()]));

        let value = evaluate(
            &ast,
            &mut |name| match name {
                "V1.Max" => Ok(Value::Int(10)),
                other => Err(EvalError::UnknownVariable(other.to_string())),
            },
            EvalMode::Float,
        )
        .expect("evaluate");
        assert_eq!(value, Value::Float(9.0));
        assert!(matches!(
            parse_expression("1.5").expect("parse"),
            AstNode::Literal(Value::Float(v)) if v == 1.5
        ));
    }

    #[test]
    fn var_attr_parses_the_standard_extensions_only() {
        assert_eq!(VarAttr::parse("Value"), Some(VarAttr::Value));
        assert_eq!(VarAttr::parse("Min"), Some(VarAttr::Min));
        assert_eq!(VarAttr::parse("MAX"), Some(VarAttr::Max));
        assert_eq!(VarAttr::parse("inc"), Some(VarAttr::Inc));
        assert_eq!(
            VarAttr::parse("Entry.Continuous"),
            Some(VarAttr::Entry("Continuous".into()))
        );
        for bad in [
            "",
            "Minimum",
            "Entry",
            "Entry.",
            "Entry.A.B",
            "Min.Max",
            "Value.X",
        ] {
            assert_eq!(VarAttr::parse(bad), None, "{bad:?}");
        }
        assert_eq!(split_qualified("V1.Min"), Some(("V1", "Min")));
        assert_eq!(split_qualified("V1"), None);
    }

    #[test]
    fn parse_error_reports_offset() {
        let err = parse_expression("A + ").expect_err("incomplete expression");
        assert!(err.offset() > 0, "offset should point past the operator");
        assert!(err.to_string().contains("at offset"));
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
