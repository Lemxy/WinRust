//! The expression evaluator and the number formatting of rs-calc, shared by the window and the command line.
//!
//! A recursive-descent parser, one function per precedence level:
//! `expr -> term -> unary -> power -> postfix -> primary`. It accepts `+ - * / % ^`, parentheses, unary signs,
//! factorial, the constants `pi`/`e` and the usual functions (`sin cos tan asin acos atan ln log sqrt`, besides
//! `abs` and `exp` for pasted expressions). Numbers are `f64`; a result that is not finite is an error.
//!
//! `%` is a percent sign after a complete value (`50%` is 0.5) and the remainder operator when an operand follows
//! (`7%3` is 1), so both `200 * 10%` and scripts that use `%` as modulo read naturally.

/// What can go wrong: a malformed expression, or a mathematical dead end (division by zero, `sqrt(-1)`, `ln(0)`...).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Syntax,
    Domain,
}

/// The unit the trigonometric functions work in. The window has a DEG/RAD chip; the command line takes `--rad`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Angle {
    Deg,
    Rad,
}

/// Evaluates an expression. The decimal separator may be `.` or `,`; `×`, `÷`, `−`, `√` and `π` are accepted too.
pub fn eval(src: &str, angle: Angle) -> Result<f64, Error> {
    let s = normalize(src);
    let mut p = P { s: s.as_bytes(), i: 0, angle };
    let v = p.expr()?;
    if p.i != p.s.len() {
        return Err(Error::Syntax); // trailing junk, an unclosed value, a missing operator
    }
    finite(v)
}

/// One function of the scientific panel, applied to `x`.
pub fn apply(name: &str, x: f64, angle: Angle) -> Result<f64, Error> {
    let v = match name {
        "sin" => to_rad(x, angle).sin(),
        "cos" => to_rad(x, angle).cos(),
        "tan" => to_rad(x, angle).tan(),
        "asin" if (-1.0..=1.0).contains(&x) => from_rad(x.asin(), angle),
        "acos" if (-1.0..=1.0).contains(&x) => from_rad(x.acos(), angle),
        "atan" => from_rad(x.atan(), angle),
        "ln" if x > 0.0 => x.ln(),
        "log" if x > 0.0 => x.log10(),
        "sqrt" if x >= 0.0 => x.sqrt(),
        "abs" => x.abs(),
        "exp" => x.exp(),
        _ => return Err(Error::Domain),
    };
    finite(v)
}

/// `n!` for a non-negative integer that fits in an f64 (171! overflows). Anything else is a domain error.
pub fn factorial(x: f64) -> Result<f64, Error> {
    if !x.is_finite() || x < 0.0 || (x - x.round()).abs() > 1e-9 || x > 170.0 {
        return Err(Error::Domain);
    }
    let n = x.round() as u32;
    let mut r = 1.0;
    for i in 2..=n {
        r *= i as f64;
    }
    finite(r)
}

/// Up to 12 significant digits, no trailing zeros, scientific notation outside 1e-6..1e12. `None` = not finite.
/// `sep` is the decimal separator the caller wants (the window uses the language's; the command line always `.`).
pub fn format(v: f64, sep: char) -> Option<String> {
    if !v.is_finite() {
        return None;
    }
    if v == 0.0 {
        return Some("0".into()); // also turns -0 into 0
    }
    let a = v.abs();
    let mut s = if a >= 1e12 || a < 1e-6 {
        let raw = format!("{a:.11e}"); // 12 significant digits
        let (mantissa, exp) = raw.split_once('e')?;
        let mut mantissa = mantissa.to_string();
        while mantissa.ends_with('0') {
            mantissa.pop();
        }
        if mantissa.ends_with('.') {
            mantissa.pop();
        }
        let exp: i32 = exp.parse().ok()?;
        format!("{mantissa}e{}{}", if exp < 0 { "-" } else { "+" }, exp.abs())
    } else {
        let exp = a.log10().floor() as i32;
        let decimals = (11 - exp).clamp(0, 15) as usize;
        let mut s = format!("{:.1$}", a, decimals);
        if s.contains('.') {
            while s.ends_with('0') {
                s.pop();
            }
            if s.ends_with('.') {
                s.pop();
            }
        }
        s
    };
    if v < 0.0 {
        s.insert(0, '-');
    }
    if sep != '.' {
        s = s.replace('.', &sep.to_string());
    }
    Some(s)
}

fn finite(v: f64) -> Result<f64, Error> {
    if v.is_finite() { Ok(v) } else { Err(Error::Domain) }
}

fn to_rad(x: f64, angle: Angle) -> f64 {
    match angle {
        Angle::Deg => x.to_radians(),
        Angle::Rad => x,
    }
}

fn from_rad(x: f64, angle: Angle) -> f64 {
    match angle {
        Angle::Deg => x.to_degrees(),
        Angle::Rad => x,
    }
}

/// Folds the characters a keyboard, a paste or the window's own labels can produce into the ASCII the parser reads.
fn normalize(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 8);
    for ch in src.chars() {
        match ch {
            '×' | '·' | '⋅' => out.push('*'),
            '÷' | '∕' => out.push('/'),
            '−' | '–' | '—' => out.push('-'),
            '√' => out.push_str("sqrt"),
            'π' => out.push_str("pi"),
            '²' => out.push_str("^2"),
            ',' | '，' => out.push('.'),
            c if c.is_whitespace() => {}
            c if c.is_ascii() => out.push(c.to_ascii_lowercase()),
            _ => out.push('\u{fffd}'), // unknown: primary() reports a syntax error
        }
    }
    out
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
    angle: Angle,
}

impl P<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn digit(&mut self) -> bool {
        if self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Result<f64, Error> {
        let mut v = self.term()?;
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.i += 1;
                    v += self.term()?;
                }
                Some(b'-') => {
                    self.i += 1;
                    v -= self.term()?;
                }
                _ => return Ok(v),
            }
        }
    }

    fn term(&mut self) -> Result<f64, Error> {
        let mut v = self.unary()?;
        loop {
            match self.peek() {
                Some(b'*') => {
                    self.i += 1;
                    v *= self.unary()?;
                }
                Some(b'/') => {
                    self.i += 1;
                    let d = self.unary()?;
                    if d == 0.0 {
                        return Err(Error::Domain);
                    }
                    v /= d;
                }
                Some(b'%') if !self.percent_is_postfix() => {
                    self.i += 1;
                    let d = self.unary()?;
                    if d == 0.0 {
                        return Err(Error::Domain);
                    }
                    v %= d;
                }
                _ => return Ok(v),
            }
        }
    }

    fn unary(&mut self) -> Result<f64, Error> {
        if self.eat(b'+') {
            return self.unary();
        }
        if self.eat(b'-') {
            return Ok(-self.unary()?);
        }
        self.power()
    }

    fn power(&mut self) -> Result<f64, Error> {
        let base = self.postfix()?;
        if self.eat(b'^') {
            // right-associative, and the exponent may carry its own sign: 2^-3, 2^3^2 = 2^(3^2)
            return Ok(base.powf(self.unary()?));
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Result<f64, Error> {
        let mut v = self.primary()?;
        loop {
            if self.eat(b'!') {
                v = factorial(v)?;
            } else if self.peek() == Some(b'%') && self.percent_is_postfix() {
                self.i += 1;
                v /= 100.0;
            } else {
                return Ok(v);
            }
        }
    }

    /// At `%`: true when it closes the value (a percent sign) instead of being a remainder with a right operand.
    fn percent_is_postfix(&self) -> bool {
        !matches!(self.s.get(self.i + 1), Some(b'0'..=b'9' | b'.' | b'(' | b'a'..=b'z'))
    }

    fn primary(&mut self) -> Result<f64, Error> {
        match self.peek() {
            Some(b'(') => {
                self.i += 1;
                let v = self.expr()?;
                if !self.eat(b')') {
                    return Err(Error::Syntax);
                }
                Ok(v)
            }
            Some(b'0'..=b'9' | b'.') => self.number().ok_or(Error::Syntax),
            Some(b'a'..=b'z') => {
                let start = self.i;
                while self.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
                    self.i += 1;
                }
                let name = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| Error::Syntax)?;
                match name {
                    "pi" => return Ok(std::f64::consts::PI),
                    "e" => return Ok(std::f64::consts::E),
                    _ => {}
                }
                let x = if self.eat(b'(') {
                    let v = self.expr()?;
                    if !self.eat(b')') {
                        return Err(Error::Syntax);
                    }
                    v
                } else {
                    // sqrt9 and sin30: a function may be followed by a bare value
                    self.unary()?
                };
                apply(name, x, self.angle)
            }
            _ => Err(Error::Syntax),
        }
    }

    /// A decimal number with an optional `e`/`E` exponent. It only takes the `e` when digits follow, so `2e` still
    /// leaves the constant `e` for the parser.
    fn number(&mut self) -> Option<f64> {
        let start = self.i;
        let mut digits = 0;
        while self.digit() {
            digits += 1;
        }
        if self.eat(b'.') {
            while self.digit() {
                digits += 1;
            }
        }
        if digits == 0 {
            return None;
        }
        if self.peek() == Some(b'e') {
            let save = self.i;
            self.i += 1;
            let _ = self.eat(b'+') || self.eat(b'-');
            if self.digit() {
                while self.digit() {}
            } else {
                self.i = save; // not an exponent: leave the e for the identifier branch
            }
        }
        std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(got: Result<f64, Error>, want: f64) {
        let got = got.expect("expression should evaluate");
        assert!((got - want).abs() <= 1e-9 * want.abs().max(1.0), "{got} != {want}");
    }

    #[test]
    fn precedence_and_parentheses() {
        close(eval("2+2*3", Angle::Rad), 8.0);
        close(eval("(2+2)*3", Angle::Rad), 12.0);
        close(eval("10/4-1", Angle::Rad), 1.5);
        close(eval("2^3^2", Angle::Rad), 512.0);
        close(eval("-2^2", Angle::Rad), -4.0);
        close(eval("2^-2", Angle::Rad), 0.25);
    }

    #[test]
    fn percent_and_modulo() {
        close(eval("50%", Angle::Rad), 0.5);
        close(eval("7%3", Angle::Rad), 1.0);
        close(eval("7 % 3", Angle::Rad), 1.0);
        close(eval("200*10%", Angle::Rad), 20.0);
        close(eval("5!%", Angle::Rad), 1.2);
    }

    #[test]
    fn functions_and_constants() {
        close(eval("sin(30)", Angle::Deg), 0.5);
        close(eval("cos(60)", Angle::Deg), 0.5);
        close(eval("asin(0.5)", Angle::Deg), 30.0);
        close(eval("sin(pi/2)", Angle::Rad), 1.0);
        close(eval("ln(e)", Angle::Rad), 1.0);
        close(eval("log(1000)", Angle::Rad), 3.0);
        close(eval("sqrt(9)", Angle::Rad), 3.0);
        close(eval("√9", Angle::Rad), 3.0);
        close(eval("sqrt9", Angle::Rad), 3.0);
        close(eval("5!", Angle::Rad), 120.0);
        close(eval("2×3÷4", Angle::Rad), 1.5);
        close(eval("1,5+2", Angle::Rad), 3.5);
    }

    #[test]
    fn bad_input_is_an_error() {
        assert_eq!(eval("", Angle::Rad), Err(Error::Syntax));
        assert_eq!(eval("2+", Angle::Rad), Err(Error::Syntax));
        assert_eq!(eval("(2+3", Angle::Rad), Err(Error::Syntax));
        assert_eq!(eval("2(3)", Angle::Rad), Err(Error::Syntax));
        assert_eq!(eval("1/0", Angle::Rad), Err(Error::Domain));
        assert_eq!(eval("ln(0)", Angle::Rad), Err(Error::Domain));
        assert_eq!(eval("sqrt(-1)", Angle::Rad), Err(Error::Domain));
        assert_eq!(eval("asin(2)", Angle::Rad), Err(Error::Domain));
        assert_eq!(eval("(-1)!", Angle::Rad), Err(Error::Domain));
        assert_eq!(eval("2.5!", Angle::Rad), Err(Error::Domain));
    }

    #[test]
    fn formatting_keeps_twelve_significant_digits() {
        assert_eq!(format(8.0, '.').as_deref(), Some("8"));
        assert_eq!(format(-0.0, '.').as_deref(), Some("0"));
        assert_eq!(format(2.5, ',').as_deref(), Some("2,5"));
        assert_eq!(format(1.0 / 3.0, '.').as_deref(), Some("0.333333333333"));
        assert_eq!(format(2.0 / 3.0, '.').as_deref(), Some("0.666666666667"));
        assert_eq!(format(0.1 + 0.2, '.').as_deref(), Some("0.3"));
        assert_eq!(format(123456789012.0, '.').as_deref(), Some("123456789012"));
        assert_eq!(format(1e12, '.').as_deref(), Some("1e+12"));
        assert_eq!(format(1e-7, '.').as_deref(), Some("1e-7"));
        assert_eq!(format(-1.25e-8, '.').as_deref(), Some("-1.25e-8"));
        assert_eq!(format(f64::NAN, '.'), None);
        assert_eq!(format(f64::INFINITY, '.'), None);
    }
}
