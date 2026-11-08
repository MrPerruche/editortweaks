//! Lengths typed into the editor's text boxes.
//!
//! The Brick Rigs editor mod lets players type an exact length (a brick face's size, a line's
//! length) in the unit the editor shows: `m`, `cm`, or brick units with no unit text at all. The
//! text may be a small math formula, like the game's own slider boxes accept.
//!
//! - [`evaluate`] reads a formula such as `2(x+1)^2 / 3`.
//! - [`typed_length`] turns what the player typed into the length they ask for, handling the shown
//!   unit and relative forms like `-20` or `*=2`.
//! - [`typed_angle`] does the same for an angle in degrees (a turn about the line tool's line),
//!   which may be negative; [`angle_text`] writes one.
//! - [`cm_per_unit`] finds the exact size of the shown unit behind the game's float factor.
//!
//! No game code here: plain text in, plain numbers out, so it is fully unit tested.

/// Evaluates a formula with the value `x`.
///
/// Supported syntax, from loosest to tightest binding:
///
/// - `+` and `-` (binary)
/// - `*`, `/`, `%` (remainder, like `f64`'s `%`) and implicit multiplication
/// - unary `+` and `-`
/// - `^` (power, right-associative, so `2^3^2` is `2^9`; `-2^2` is `-4`; the exponent may be
///   negative: `2^-1` is `0.5`)
/// - numbers (`.` or `,` as the decimal point, `.5` allowed), `x` / `X`, `( )` and `sqrt`
///
/// `sqrt` applies to the power-level operand right after it: `sqrt(4)`, `sqrt 4`, `sqrt x^2` (which
/// is `sqrt(x^2)`).
///
/// Implicit multiplication (same precedence as `*`) happens when an operand is directly followed
/// by `x`, `(` or `sqrt`, or by a number after `)` or `x`: `2x`, `x(1+x)`, `2(3)`, `(1)(2)`,
/// `x2`. Two plain numbers in a row (`2 3`) are an error. Whitespace is ignored between tokens.
///
/// Returns `None` for a syntax error, an unknown character or word, empty text, and any
/// non-finite result (division by zero, `sqrt` of a negative number).
pub fn evaluate(text: &str, x: f64) -> Option<f64> {
    let tokens = tokenize(text)?;
    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
        x,
        depth: 0,
    };
    let value = parser.expression()?;
    // Everything must be used up: "1)" leaves a stray token behind.
    if parser.pos != tokens.len() {
        return None;
    }
    finite(value)
}

/// The length (in the shown unit) that a typed text asks for; `now` is the current length in the
/// same unit.
///
/// 1. Every occurrence of `unit` is removed from the text first (case-sensitive; skipped when the
///    unit is blank), so a prefilled `1.5 m` or `1.5 m + 20` works.
/// 2. A prefix `+=`, `-=`, `*=` or `/=` applies the rest to `now`: `*=2` doubles it.
/// 3. A leading `+` or `-` makes the text relative: `-20` with `now` 50 is 30.
/// 4. Anything else is the length itself.
///
/// In every formula `x` is `now`. Returns `None` for unreadable text, or a result that is not
/// finite or is negative.
pub fn typed_length(text: &str, unit: &str, now: f64) -> Option<f64> {
    typed_value(text, unit, now, Signs::Relative).filter(|length| *length >= 0.0)
}

/// The angle (degrees) that a typed text asks for, `now` being the angle now (degrees): read like
/// [`typed_length`] (`°` left out, `+=`, `-=`, `*=`, `/=` apply to `now`, `x` is `now`), except
/// that a leading `+` or `-` is the number's sign (`-20` is -20 degrees, an angle may be negative).
/// `None` for unreadable text or a result that isn't finite.
pub fn typed_angle(text: &str, now: f64) -> Option<f64> {
    typed_value(text, DEGREE, now, Signs::Absolute)
}

/// What a leading `+` or `-` means in a typed value.
#[derive(Clone, Copy, PartialEq)]
enum Signs {
    /// Adds to the value now, or takes from it (lengths: they can't be negative).
    Relative,
    /// The number's sign (angles).
    Absolute,
}

/// The degree sign, shown after angles (see [`angle_text`]) and left out of typed ones.
pub const DEGREE: &str = "°";

/// How an angle (degrees) is written: at most [`ANGLE_DECIMALS`] decimals, trailing zeros left out
/// (`35.5°`, `90°`), then the degree sign. Never `-0°`.
pub fn angle_text(degrees: f64) -> String {
    let fixed = format!("{degrees:.ANGLE_DECIMALS$}");
    let trimmed = if fixed.contains('.') {
        fixed.trim_end_matches('0').trim_end_matches('.')
    } else {
        &fixed
    };
    let number = if trimmed == "-0" { "0" } else { trimmed };
    format!("{number}{DEGREE}")
}

/// At most how many decimals [`angle_text`] writes.
pub const ANGLE_DECIMALS: usize = 3;

/// What [`typed_length`] and [`typed_angle`] share: any finite result.
fn typed_value(text: &str, unit: &str, now: f64, signs: Signs) -> Option<f64> {
    let unit = unit.trim();
    let text = if unit.is_empty() {
        text.trim().to_string()
    } else {
        text.replace(unit, "").trim().to_string()
    };
    if text.is_empty() {
        return None;
    }

    let result = if let Some((op, rest)) = compound_prefix(&text) {
        let operand = evaluate(rest, now)?;
        match op {
            '+' => now + operand,
            '-' => now - operand,
            '*' => now * operand,
            _ => now / operand,
        }
    } else if signs == Signs::Relative && text.starts_with(['+', '-']) {
        // The sign stays part of the formula: "-20" is evaluated as minus twenty.
        now + evaluate(&text, now)?
    } else {
        evaluate(&text, now)?
    };

    finite(result)
}

/// How many cm one shown unit is, from the game's `factor` (cm to the unit, a float): the shortest
/// decimal whose inverse is that float, so the unit's real size (`30` for brick units, `100` for
/// m, `2.54` for inches) rather than the float's rounding of it. Lengths typed are multiplied by
/// it: `2` brick units are then exactly 60 cm (not 59.999996), and `1/3` m keeps every digit.
/// The float's own inverse if no decimal of up to 17 digits is (or `factor` isn't positive).
pub fn cm_per_unit(factor: f32) -> f64 {
    let inverse = 1.0 / f64::from(factor);
    if !(inverse.is_finite() && inverse > 0.0) {
        return inverse;
    }
    (0..17)
        .filter_map(|decimals| format!("{inverse:.decimals$e}").parse::<f64>().ok())
        .find(|&cm| (1.0 / cm) as f32 == factor)
        .unwrap_or(inverse)
}

/// Splits a leading `+=`, `-=`, `*=` or `/=` off `text`: the operator and what follows.
fn compound_prefix(text: &str) -> Option<(char, &str)> {
    let mut chars = text.chars();
    let op = chars
        .next()
        .filter(|c| matches!(c, '+' | '-' | '*' | '/'))?;
    let rest = chars.as_str().strip_prefix('=')?;
    Some((op, rest))
}

/// `Some(value)` when it is a real number, `None` for infinity and NaN.
fn finite(value: f64) -> Option<f64> {
    value.is_finite().then_some(value)
}

/// One piece of a formula.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Token {
    Number(f64),
    /// `x` or `X`.
    X,
    Sqrt,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    OpenParen,
    CloseParen,
}

/// Splits a formula into tokens, skipping whitespace. `None` on an unknown character or word.
fn tokenize(text: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut rest = text;

    while let Some(c) = rest.chars().next() {
        if c.is_whitespace() {
            rest = &rest[c.len_utf8()..];
        } else if c.is_ascii_digit() || c == '.' || c == ',' {
            let (number, after) = read_number(rest)?;
            tokens.push(Token::Number(number));
            rest = after;
        } else if let Some(after) = rest.strip_prefix("sqrt") {
            tokens.push(Token::Sqrt);
            rest = after;
        } else {
            let token = match c {
                'x' | 'X' => Token::X,
                '+' => Token::Plus,
                '-' => Token::Minus,
                '*' => Token::Star,
                '/' => Token::Slash,
                '%' => Token::Percent,
                '^' => Token::Caret,
                '(' => Token::OpenParen,
                ')' => Token::CloseParen,
                _ => return None,
            };
            tokens.push(token);
            rest = &rest[c.len_utf8()..];
        }
    }
    Some(tokens)
}

/// Reads a number at the start of `text` (digits with at most one `.` or `,` as the point) and
/// returns it with the text after it. `None` when there is no digit (a lone point).
fn read_number(text: &str) -> Option<(f64, &str)> {
    let mut seen_point = false;
    let end = text
        .find(|c: char| {
            if c == '.' || c == ',' {
                // The first point belongs to the number, a second one ends it.
                std::mem::replace(&mut seen_point, true)
            } else {
                !c.is_ascii_digit()
            }
        })
        .unwrap_or(text.len());

    let (digits, after) = text.split_at(end);
    if !digits.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let number = digits.replace(',', ".").parse().ok()?;
    Some((number, after))
}

/// Deepest nesting (parentheses, signs, powers) we accept, so absurd input can't overflow the
/// stack.
const MAX_DEPTH: usize = 100;

/// Recursive descent parser that evaluates while it parses.
///
/// Grammar (each rule is one method):
///
/// ```text
/// expression = term (("+" | "-") term)*
/// term       = unary (("*" | "/" | "%")? unary)*     (no operator = implicit multiplication)
/// unary      = ("+" | "-") unary | power
/// power      = primary ("^" unary)?
/// primary    = number | "x" | "(" expression ")" | "sqrt" power
/// ```
struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// The value of `x`.
    x: f64,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<Token> {
        self.tokens.get(self.pos).copied()
    }

    /// Consumes the next token if it is `token`.
    fn eat(&mut self, token: Token) -> bool {
        let found = self.peek() == Some(token);
        if found {
            self.pos += 1;
        }
        found
    }

    /// Counts one more level of nesting; `None` when it is too deep. Pair with `leave`.
    fn enter(&mut self) -> Option<()> {
        self.depth += 1;
        (self.depth <= MAX_DEPTH).then_some(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn expression(&mut self) -> Option<f64> {
        self.enter()?;
        let mut value = self.term()?;
        loop {
            if self.eat(Token::Plus) {
                value = finite(value + self.term()?)?;
            } else if self.eat(Token::Minus) {
                value = finite(value - self.term()?)?;
            } else {
                break;
            }
        }
        self.leave();
        Some(value)
    }

    fn term(&mut self) -> Option<f64> {
        let mut value = self.unary()?;
        loop {
            if self.eat(Token::Star) {
                value = finite(value * self.unary()?)?;
            } else if self.eat(Token::Slash) {
                value = finite(value / self.unary()?)?;
            } else if self.eat(Token::Percent) {
                value = finite(value % self.unary()?)?;
            } else if self.implicit_multiplication_next() {
                value = finite(value * self.unary()?)?;
            } else {
                break;
            }
        }
        Some(value)
    }

    /// Whether the next token starts an operand that multiplies the previous one without an
    /// operator: `x`, `(` or `sqrt` always do, a number only right after `)` or `x`.
    fn implicit_multiplication_next(&self) -> bool {
        match self.peek() {
            Some(Token::X | Token::OpenParen | Token::Sqrt) => true,
            Some(Token::Number(_)) => {
                let previous = self.pos.checked_sub(1).map(|i| self.tokens[i]);
                matches!(previous, Some(Token::CloseParen | Token::X))
            }
            _ => false,
        }
    }

    fn unary(&mut self) -> Option<f64> {
        self.enter()?;
        let value = if self.eat(Token::Minus) {
            -self.unary()?
        } else if self.eat(Token::Plus) {
            self.unary()?
        } else {
            self.power()?
        };
        self.leave();
        Some(value)
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.primary()?;
        if self.eat(Token::Caret) {
            // The exponent is a `unary`, which makes `^` right-associative and allows `2^-1`.
            let exponent = self.unary()?;
            finite(base.powf(exponent))
        } else {
            Some(base)
        }
    }

    fn primary(&mut self) -> Option<f64> {
        let token = self.peek()?;
        self.pos += 1;
        match token {
            Token::Number(number) => Some(number),
            Token::X => Some(self.x),
            Token::OpenParen => {
                let value = self.expression()?;
                self.eat(Token::CloseParen).then_some(value)
            }
            // `sqrt` of a negative number is NaN, which `finite` turns into `None`.
            Token::Sqrt => {
                self.enter()?;
                let operand = self.power()?;
                self.leave();
                finite(operand.sqrt())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_sizes_are_exact() {
        assert_eq!(cm_per_unit(1.0 / 30.0), 30.0);
        assert_eq!(cm_per_unit(0.01), 100.0);
        assert_eq!(cm_per_unit(1.0), 1.0);
        assert_eq!(cm_per_unit(1.0 / 2.54), 2.54);
        // Lengths typed in them.
        assert_eq!(2.0 * cm_per_unit(1.0 / 30.0), 60.0);
        assert_eq!(
            (100.0 / 3.0) as f32,
            ((1.0 / 3.0) * cm_per_unit(0.01)) as f32
        );
    }

    #[test]
    fn odd_factors_still_invert() {
        let factor = 0.123_456_79_f32;
        let cm = cm_per_unit(factor);
        assert_eq!((1.0 / cm) as f32, factor);
        assert!(cm_per_unit(0.0).is_infinite());
    }

    /// Floating point results are compared with a small tolerance.
    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// `evaluate(text, x)` must be `Some(expected)`.
    fn check_eval(text: &str, x: f64, expected: f64) {
        match evaluate(text, x) {
            Some(value) => assert!(
                close(value, expected),
                "{text:?} (x={x}): {value} != {expected}"
            ),
            None => panic!("{text:?} (x={x}) was rejected, expected {expected}"),
        }
    }

    fn check_eval_rejected(text: &str) {
        assert_eq!(evaluate(text, 7.0), None, "{text:?} should be rejected");
    }

    /// `typed_length(text, unit, now)` must be `Some(expected)`.
    fn check_length(text: &str, unit: &str, now: f64, expected: f64) {
        match typed_length(text, unit, now) {
            Some(value) => assert!(
                close(value, expected),
                "{text:?} (unit {unit:?}, now={now}): {value} != {expected}"
            ),
            None => panic!("{text:?} (unit {unit:?}, now={now}) was rejected, expected {expected}"),
        }
    }

    fn check_length_rejected(text: &str, unit: &str, now: f64) {
        assert_eq!(
            typed_length(text, unit, now),
            None,
            "{text:?} (unit {unit:?}, now={now}) should be rejected"
        );
    }

    // ---- evaluate: numbers ----

    #[test]
    fn plain_numbers() {
        check_eval("0", 0.0, 0.0);
        check_eval("42", 0.0, 42.0);
        check_eval("1.5", 0.0, 1.5);
        check_eval("1.", 0.0, 1.0);
        check_eval("007", 0.0, 7.0);
    }

    #[test]
    fn leading_point() {
        check_eval(".5", 0.0, 0.5);
        check_eval(",5", 0.0, 0.5);
        check_eval(".5+.25", 0.0, 0.75);
    }

    #[test]
    fn comma_is_a_decimal_point() {
        check_eval("1,5", 0.0, 1.5);
        check_eval("1,5 + 2,25", 0.0, 3.75);
        check_eval("3,5*2", 0.0, 7.0);
    }

    #[test]
    fn x_in_both_cases() {
        check_eval("x", 3.0, 3.0);
        check_eval("X", 3.0, 3.0);
        check_eval("x+X", 3.0, 6.0);
        check_eval("x", -2.5, -2.5);
    }

    // ---- evaluate: operators ----

    #[test]
    fn basic_binary_operators() {
        check_eval("1+2", 0.0, 3.0);
        check_eval("5-8", 0.0, -3.0);
        check_eval("3*4", 0.0, 12.0);
        check_eval("10/4", 0.0, 2.5);
        check_eval("10%4", 0.0, 2.0);
        check_eval("2^10", 0.0, 1024.0);
    }

    #[test]
    fn remainder_matches_f64() {
        check_eval("7.5%2", 0.0, 1.5);
        check_eval("-7%3", 0.0, -1.0);
    }

    #[test]
    fn precedence() {
        check_eval("1+2*3", 0.0, 7.0);
        check_eval("2*3+1", 0.0, 7.0);
        check_eval("10-6/3", 0.0, 8.0);
        check_eval("2*3^2", 0.0, 18.0);
        check_eval("10+7%4", 0.0, 13.0);
        check_eval("(1+2)*3", 0.0, 9.0);
    }

    #[test]
    fn left_associativity() {
        check_eval("10-3-2", 0.0, 5.0);
        check_eval("100/10/5", 0.0, 2.0);
        check_eval("20%7%4", 0.0, 2.0);
        check_eval("8/2*3", 0.0, 12.0);
    }

    #[test]
    fn power_is_right_associative() {
        check_eval("2^3^2", 0.0, 512.0);
        check_eval("(2^3)^2", 0.0, 64.0);
        check_eval("2^2^3", 0.0, 256.0);
    }

    #[test]
    fn power_binds_tighter_than_unary_minus() {
        check_eval("-2^2", 0.0, -4.0);
        check_eval("(-2)^2", 0.0, 4.0);
        check_eval("--2^2", 0.0, 4.0);
        check_eval("-x^2", 3.0, -9.0);
    }

    #[test]
    fn negative_exponent() {
        check_eval("2^-1", 0.0, 0.5);
        check_eval("2^+3", 0.0, 8.0);
        check_eval("2^-2^2", 0.0, 0.0625);
        check_eval("4^-.5", 0.0, 0.5);
    }

    #[test]
    fn unary_signs() {
        check_eval("-5", 0.0, -5.0);
        check_eval("+5", 0.0, 5.0);
        check_eval("--5", 0.0, 5.0);
        check_eval("-+5", 0.0, -5.0);
        check_eval("3--2", 0.0, 5.0);
        check_eval("3+-2", 0.0, 1.0);
        check_eval("2*-3", 0.0, -6.0);
        check_eval("-(1+2)", 0.0, -3.0);
    }

    #[test]
    fn parentheses() {
        check_eval("((1))", 0.0, 1.0);
        check_eval("2*(3+(4-1))", 0.0, 12.0);
        check_eval("( 1 + 2 ) * 3", 0.0, 9.0);
    }

    // ---- evaluate: sqrt ----

    #[test]
    fn sqrt_with_and_without_parentheses() {
        check_eval("sqrt(4)", 0.0, 2.0);
        check_eval("sqrt 4", 0.0, 2.0);
        check_eval("sqrt(2+2)", 0.0, 2.0);
        check_eval("sqrt x", 9.0, 3.0);
        check_eval("sqrt(0)", 0.0, 0.0);
    }

    #[test]
    fn sqrt_takes_the_power_operand() {
        // `sqrt x^2` is sqrt(x^2), not (sqrt x)^2: the same for positive x, differs for negative.
        check_eval("sqrt x^2", -3.0, 3.0);
        check_eval("sqrt 2^2", 0.0, 2.0);
        // Anything after the operand's power is outside the sqrt.
        check_eval("sqrt 16 + 1", 0.0, 5.0);
        check_eval("sqrt 16 * 2", 0.0, 8.0);
        check_eval("2^sqrt 4", 0.0, 4.0);
    }

    #[test]
    fn sqrt_negative_is_rejected() {
        check_eval_rejected("sqrt(-4)");
        check_eval_rejected("sqrt(1-2)");
    }

    // ---- evaluate: implicit multiplication ----

    #[test]
    fn number_then_x() {
        check_eval("2x", 5.0, 10.0);
        check_eval("2X", 5.0, 10.0);
        check_eval("2 x", 5.0, 10.0);
        check_eval("1.5x", 4.0, 6.0);
        check_eval("xx", 3.0, 9.0);
    }

    #[test]
    fn x_then_parenthesis() {
        check_eval("x(1+x)", 3.0, 12.0);
        check_eval("x (2)", 3.0, 6.0);
    }

    #[test]
    fn number_then_parenthesis() {
        check_eval("2(3)", 0.0, 6.0);
        check_eval("2(3+1)", 0.0, 8.0);
    }

    #[test]
    fn parenthesis_then_parenthesis() {
        check_eval("(1)(2)", 0.0, 2.0);
        check_eval("(1+1)(2+1)(3+1)", 0.0, 24.0);
    }

    #[test]
    fn number_after_x_or_parenthesis() {
        check_eval("x2", 5.0, 10.0);
        check_eval("x 2", 5.0, 10.0);
        check_eval("(1+1)3", 0.0, 6.0);
        check_eval("2x3", 5.0, 30.0);
    }

    #[test]
    fn implicit_multiplication_with_sqrt() {
        check_eval("2sqrt 4", 0.0, 4.0);
        check_eval("2 sqrt(9)", 0.0, 6.0);
        check_eval("x sqrt 4", 3.0, 6.0);
        check_eval("(2)sqrt 9", 0.0, 6.0);
        check_eval("sqrt(4)2", 0.0, 4.0);
        check_eval("sqrt(4)(3)", 0.0, 6.0);
        check_eval("sqrt(4)x", 3.0, 6.0);
    }

    #[test]
    fn implicit_multiplication_has_the_precedence_of_multiplication() {
        // Not `2 * (x^2)` versus `(2x)^2`: the power binds to its own operand.
        check_eval("2x^2", 3.0, 18.0);
        check_eval("1+2x", 3.0, 7.0);
        check_eval("10/2x", 3.0, 15.0); // left to right: (10/2)*x
        check_eval("6/2(3)", 0.0, 9.0);
        check_eval("-2x", 3.0, -6.0);
        check_eval("2x%4", 3.0, 2.0); // (2*3) % 4
    }

    #[test]
    fn number_after_number_is_not_implicit() {
        check_eval_rejected("2 3");
        check_eval_rejected("1..2");
        check_eval_rejected("2^3 4");
        check_eval_rejected("sqrt 4 2");
        check_eval_rejected("1.2.3");
    }

    // ---- evaluate: whitespace ----

    #[test]
    fn whitespace_is_ignored() {
        check_eval("  1  +  2  ", 0.0, 3.0);
        check_eval("\t2\n*\t3 ", 0.0, 6.0);
        check_eval(" - 2 ^ 2 ", 0.0, -4.0);
        check_eval(" sqrt ( 4 ) ", 0.0, 2.0);
    }

    // ---- evaluate: rejected input ----

    #[test]
    fn garbage_is_rejected() {
        for text in [
            "abc", "1..2", "(1", "1)", "", "   ", "+", "-", "sqrt", "2*/3", "()", "1+", "*2", "2*",
            "1 2", "1+*2", "1^", "^2", "sqrt()", "y", "2y", "1 m", "1;2", "1+(2", "((1)", ")(",
            "=", "1=1", ".", ",", "..", "sqrtt 4", "Sqrt 4", "SQRT(4)", "2²", "€5",
        ] {
            check_eval_rejected(text);
        }
    }

    #[test]
    fn division_by_zero_is_rejected() {
        check_eval_rejected("1/0");
        check_eval_rejected("0/0");
        check_eval_rejected("1/(1-1)");
        check_eval_rejected("5%0");
        // An infinity in the middle must not be hidden by a later operation.
        check_eval_rejected("1/(1/0)");
        check_eval_rejected("0*(1/0)");
        assert_eq!(evaluate("1/x", 0.0), None);
        assert_eq!(evaluate("5%x", 0.0), None);
    }

    #[test]
    fn non_finite_results_are_rejected() {
        check_eval_rejected("0^-1");
        check_eval_rejected("10^400");
        check_eval_rejected("(-8)^.5");
        assert_eq!(evaluate("x", f64::NAN), None);
        assert_eq!(evaluate("x", f64::INFINITY), None);
    }

    #[test]
    fn absurd_nesting_is_rejected_instead_of_overflowing_the_stack() {
        let deep = format!("{}1{}", "(".repeat(5000), ")".repeat(5000));
        check_eval_rejected(&deep);
        check_eval_rejected(&format!("{}1", "-".repeat(5000)));
        check_eval_rejected(&format!("{}1", "2^".repeat(5000)));
        check_eval_rejected(&format!("{}1", "sqrt ".repeat(5000)));
        // Reasonable nesting still works.
        let fine = format!("{}1{}", "(".repeat(30), ")".repeat(30));
        check_eval(&fine, 0.0, 1.0);
    }

    #[test]
    fn non_ascii_text_is_rejected_without_panicking() {
        check_eval_rejected("é");
        check_eval_rejected("1+é");
        check_eval_rejected("日本語");
    }

    // ---- typed_length: units ----

    #[test]
    fn plain_length_without_unit() {
        check_length("1.5", "", 10.0, 1.5);
        check_length("  25  ", "", 10.0, 25.0);
        check_length("0", "", 10.0, 0.0);
        check_length("2*(3+4)", "", 10.0, 14.0);
    }

    #[test]
    fn unit_is_removed() {
        check_length("1.5 m", "m", 10.0, 1.5);
        check_length("1.5m", "m", 10.0, 1.5);
        check_length("1.5 m + 20", "m", 10.0, 21.5);
        check_length("1.5m+20", "m", 10.0, 21.5);
        check_length("150 cm", "cm", 10.0, 150.0);
        check_length("1,5 m", "m", 10.0, 1.5);
    }

    #[test]
    fn unit_is_removed_everywhere_and_trimmed() {
        check_length("1 m + 2 m", "m", 10.0, 3.0);
        check_length("2 m * 3", "m", 10.0, 6.0);
        check_length("(1 m + 1 m)", "m", 10.0, 2.0);
        // The unit setting itself may carry spaces.
        check_length("1.5 m", "  m ", 10.0, 1.5);
    }

    #[test]
    fn unit_removal_is_case_sensitive() {
        check_length_rejected("1.5 M", "m", 10.0);
        check_length("1.5 M", "M", 10.0, 1.5);
    }

    #[test]
    fn empty_unit_leaves_the_text_alone() {
        check_length_rejected("1.5 m", "", 10.0);
        check_length_rejected("1.5 m", "   ", 10.0);
        check_length("1.5", "   ", 10.0, 1.5);
    }

    #[test]
    fn unit_that_does_not_appear_changes_nothing() {
        check_length("1.5", "cm", 10.0, 1.5);
        check_length("7 + 1", "m", 10.0, 8.0);
    }

    #[test]
    fn only_the_unit_is_empty() {
        check_length_rejected("m", "m", 10.0);
        check_length_rejected(" m ", "m", 10.0);
        check_length_rejected("", "m", 10.0);
        check_length_rejected("   ", "", 10.0);
    }

    // ---- typed_length: x ----

    #[test]
    fn x_is_the_current_length() {
        check_length("x", "", 12.0, 12.0);
        check_length("2x", "", 12.0, 24.0);
        check_length("x/2", "", 12.0, 6.0);
        check_length("x+x", "m", 12.0, 24.0);
        check_length("X*X", "", 3.0, 9.0);
    }

    // ---- typed_length: relative and compound forms ----

    #[test]
    fn leading_sign_is_relative() {
        check_length("-20", "", 50.0, 30.0);
        check_length("+30", "", 50.0, 80.0);
        check_length("-(5*2)", "", 50.0, 40.0);
        check_length("+2*3", "", 50.0, 56.0);
        check_length("-x/2", "", 50.0, 25.0);
        check_length("-1,5", "", 50.0, 48.5);
        check_length("-20 m", "m", 50.0, 30.0);
        check_length("+0.5 m", "m", 1.0, 1.5);
    }

    #[test]
    fn compound_forms() {
        check_length("+=5", "", 10.0, 15.0);
        check_length("-=4", "", 10.0, 6.0);
        check_length("*=3", "", 10.0, 30.0);
        check_length("/=4", "", 10.0, 2.5);
    }

    #[test]
    fn compound_forms_evaluate_the_rest_with_x_as_now() {
        check_length("+=x", "", 10.0, 20.0);
        check_length("*=x", "", 10.0, 100.0);
        check_length("-=x/2", "", 10.0, 5.0);
        check_length("/=(1+1)", "", 10.0, 5.0);
        check_length("*= 2 m", "m", 10.0, 20.0);
        check_length("  += 5", "", 10.0, 15.0);
        check_length("+=-5", "", 10.0, 5.0);
    }

    #[test]
    fn compound_forms_need_a_valid_rest() {
        check_length_rejected("+=", "", 10.0);
        check_length_rejected("*=", "", 10.0);
        check_length_rejected("/= abc", "", 10.0);
        check_length_rejected("*=*=2", "", 10.0);
    }

    #[test]
    fn equals_sign_alone_is_not_an_operator() {
        check_length_rejected("=5", "", 10.0);
        check_length_rejected("5=", "", 10.0);
    }

    // ---- typed_length: rejected results ----

    #[test]
    fn negative_results_are_rejected() {
        check_length_rejected("-60", "", 50.0);
        check_length_rejected("-=60", "", 50.0);
        check_length_rejected("*=-1", "", 50.0);
        check_length_rejected("(0-5)", "", 50.0);
        // Exactly zero is fine.
        check_length("-50", "", 50.0, 0.0);
    }

    #[test]
    fn garbage_length_is_rejected() {
        for text in [
            "abc", "1..2", "(1", "1)", "", "+", "-", "sqrt", "2*/3", "1 2",
        ] {
            check_length_rejected(text, "", 10.0);
            check_length_rejected(text, "m", 10.0);
        }
    }

    #[test]
    fn division_by_zero_length_is_rejected() {
        check_length_rejected("1/0", "", 10.0);
        check_length_rejected("/=0", "", 10.0);
        check_length_rejected("+=1/0", "", 10.0);
        check_length_rejected("/=x-10", "", 10.0);
        check_length_rejected("1/x", "", 0.0);
    }

    #[test]
    fn non_finite_now_is_rejected() {
        assert_eq!(typed_length("5", "", f64::NAN), Some(5.0));
        assert_eq!(typed_length("x", "", f64::NAN), None);
        assert_eq!(typed_length("+5", "", f64::INFINITY), None);
    }

    // ---- typed_angle ----

    #[test]
    fn plain_angle_with_or_without_degree_sign() {
        assert_eq!(typed_angle("45", 30.0), Some(45.0));
        assert_eq!(typed_angle("45°", 30.0), Some(45.0));
        assert_eq!(typed_angle(" 45 ° ", 30.0), Some(45.0));
        assert_eq!(typed_angle("22,5", 30.0), Some(22.5));
    }

    #[test]
    fn prefilled_angle_reads_back() {
        for degrees in [0.0, 12.5, 90.0, 123.456, 359.999] {
            assert_eq!(typed_angle(&angle_text(degrees), 7.0), Some(degrees));
        }
    }

    #[test]
    fn compound_angles_apply_to_now() {
        assert_eq!(typed_angle("+=15", 30.0), Some(45.0));
        assert_eq!(typed_angle("-=15°", 30.0), Some(15.0));
        assert_eq!(typed_angle("*=2", 30.0), Some(60.0));
        assert_eq!(typed_angle("/=2", 30.0), Some(15.0));
        assert_eq!(typed_angle("x/2", 30.0), Some(15.0));
        assert_eq!(typed_angle("90°+x", 30.0), Some(120.0));
        assert_eq!(typed_angle("-=50", 30.0), Some(-20.0));
    }

    #[test]
    fn a_leading_sign_is_the_angles_own() {
        // Unlike lengths, where "-20" takes 20 away.
        assert_eq!(typed_angle("-20", 30.0), Some(-20.0));
        assert_eq!(typed_angle("-20°", 30.0), Some(-20.0));
        assert_eq!(typed_angle("+20", 30.0), Some(20.0));
        assert_eq!(typed_angle("-x", 30.0), Some(-30.0));
        assert_eq!(typed_angle("-(10+5)", 30.0), Some(-15.0));
        assert_eq!(typed_angle("0-20", 30.0), Some(-20.0));
        assert_eq!(typed_length("-20", "", 30.0), Some(10.0));
        assert_eq!(typed_length("-50", "", 30.0), None);
    }

    #[test]
    fn unreadable_angles_are_rejected() {
        for text in ["", "°", "abc", "45deg", "1/0", "sqrt(-1)"] {
            assert_eq!(typed_angle(text, 30.0), None, "{text:?}");
        }
    }

    #[test]
    fn angle_text_is_short() {
        assert_eq!(angle_text(90.0), "90°");
        assert_eq!(angle_text(35.5), "35.5°");
        assert_eq!(angle_text(12.345678), "12.346°");
        assert_eq!(angle_text(0.0), "0°");
        assert_eq!(angle_text(-0.0001), "0°");
        assert_eq!(angle_text(-12.5), "-12.5°");
        assert_eq!(angle_text(360.0), "360°");
        assert_eq!(angle_text(10.0), "10°");
    }
}
