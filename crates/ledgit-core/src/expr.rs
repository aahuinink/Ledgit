//! Arithmetic for amounts: `200 * Car_Km_Rate`, `(1850 - 400) / 2`, `5%`.
//!
//! An amount field may hold an expression instead of a number. It is worked
//! out once, when the entry is made, and the entry stores the resulting
//! [`Money`] - so changing a variable later re-prices nothing already posted,
//! which is what a ledger has to promise.
//!
//! The arithmetic is exact: every value is a fraction of two `i128`s, and the
//! only rounding is the last step, to the cent, half away from zero. `0.1 +
//! 0.2` is `0.3` here, and `100 / 3 * 3` is `100`.
//!
//! ```text
//! expr    = term  (("+" | "-") term)*
//! term    = unary (("*" | "/") unary)*
//! unary   = ("-" | "+") unary | postfix
//! postfix = primary "%"*            5% is 0.05
//! primary = number | name | "(" expr ")"
//! ```
//!
//! Numbers may carry thousands commas and a `$`, as people type them. Names
//! are variables, matched without regard to case.

use crate::money::Money;
use std::fmt;

/// An exact fraction, always in lowest terms with a positive denominator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ratio {
    n: i128,
    d: i128,
}

impl Ratio {
    pub const ZERO: Ratio = Ratio { n: 0, d: 1 };
    pub const ONE: Ratio = Ratio { n: 1, d: 1 };

    pub fn new(n: i128, d: i128) -> Result<Ratio, ExprError> {
        if d == 0 {
            return Err(ExprError::new("division by zero"));
        }
        let g = gcd(n.unsigned_abs(), d.unsigned_abs()).max(1) as i128;
        let sign = if d < 0 { -1 } else { 1 };
        Ok(Ratio { n: sign * n / g, d: sign * d / g })
    }

    pub fn int(n: i64) -> Ratio {
        Ratio { n: n as i128, d: 1 }
    }

    pub fn from_money(m: Money) -> Ratio {
        Ratio::new(m.cents() as i128, 100).expect("100 is not zero")
    }

    pub fn is_negative(self) -> bool {
        self.n < 0
    }

    pub fn is_zero(self) -> bool {
        self.n == 0
    }

    pub fn checked_add(self, o: Ratio) -> Result<Ratio, ExprError> {
        let n = mul(self.n, o.d)?.checked_add(mul(o.n, self.d)?).ok_or_else(overflow)?;
        Ratio::new(n, mul(self.d, o.d)?)
    }

    pub fn checked_sub(self, o: Ratio) -> Result<Ratio, ExprError> {
        self.checked_add(Ratio { n: -o.n, d: o.d })
    }

    pub fn checked_mul(self, o: Ratio) -> Result<Ratio, ExprError> {
        // Cross-reduce first so a long chain of rates does not overflow.
        let g1 = gcd(self.n.unsigned_abs(), o.d.unsigned_abs()).max(1) as i128;
        let g2 = gcd(o.n.unsigned_abs(), self.d.unsigned_abs()).max(1) as i128;
        Ratio::new(mul(self.n / g1, o.n / g2)?, mul(self.d / g2, o.d / g1)?)
    }

    pub fn checked_div(self, o: Ratio) -> Result<Ratio, ExprError> {
        if o.n == 0 {
            return Err(ExprError::new("division by zero"));
        }
        self.checked_mul(Ratio::new(o.d, o.n)?)
    }

    /// Round to the cent, half away from zero.
    pub fn to_money(self) -> Result<Money, ExprError> {
        let scaled = mul(self.n, 100)?;
        let (q, r) = (scaled / self.d, scaled % self.d);
        let q = if 2 * r.abs() >= self.d { q + scaled.signum() } else { q };
        i64::try_from(q).map(Money).map_err(|_| overflow())
    }

    /// Parse a plain decimal: `12`, `-0.0645`, `1,200.50`, `$3`.
    pub fn parse_decimal(s: &str) -> Result<Ratio, ExprError> {
        let cleaned: String = s.chars().filter(|c| !matches!(c, ',' | '$' | '_' | ' ')).collect();
        let (neg, digits) = match cleaned.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, cleaned.strip_prefix('+').unwrap_or(&cleaned)),
        };
        let (whole, frac) = digits.split_once('.').unwrap_or((digits, ""));
        let ok = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
        if (whole.is_empty() && frac.is_empty()) || !ok(whole) || !ok(frac) || frac.len() > 18 {
            return Err(ExprError::new(format!("\"{s}\" is not a number")));
        }
        let mut n: i128 = 0;
        for b in whole.bytes().chain(frac.bytes()) {
            n = mul(n, 10)?.checked_add((b - b'0') as i128).ok_or_else(overflow)?;
        }
        let d = 10i128.pow(frac.len() as u32);
        Ratio::new(if neg { -n } else { n }, d)
    }
}

impl fmt::Display for Ratio {
    /// As a decimal when it has one of up to twelve places, else `n/d`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = self.d;
        let mut places = 0u32;
        for p in [2u32, 5] {
            while d % p as i128 == 0 {
                d /= p as i128;
            }
        }
        if d != 1 {
            return write!(f, "{}/{}", self.n, self.d);
        }
        while 10i128.pow(places) % self.d != 0 && places < 12 {
            places += 1;
        }
        if 10i128.pow(places) % self.d != 0 {
            return write!(f, "{}/{}", self.n, self.d);
        }
        let scaled = self.n * (10i128.pow(places) / self.d);
        let sign = if scaled < 0 { "-" } else { "" };
        let abs = scaled.unsigned_abs();
        if places == 0 {
            return write!(f, "{sign}{abs}");
        }
        let p = 10u128.pow(places);
        write!(f, "{sign}{}.{:0width$}", abs / p, abs % p, width = places as usize)
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn mul(a: i128, b: i128) -> Result<i128, ExprError> {
    a.checked_mul(b).ok_or_else(overflow)
}

fn overflow() -> ExprError {
    ExprError::new("the number is too large")
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExprError(pub String);

impl ExprError {
    pub fn new(msg: impl Into<String>) -> ExprError {
        ExprError(msg.into())
    }
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ExprError {}

/// Where names in an expression get their values.
pub trait Lookup {
    fn number(&self, name: &str) -> Result<Ratio, ExprError>;
}

/// No variables at all.
impl Lookup for () {
    fn number(&self, name: &str) -> Result<Ratio, ExprError> {
        Err(ExprError::new(format!("no variable called {name}")))
    }
}

#[derive(Clone, PartialEq, Debug)]
enum Tok {
    Num(Ratio),
    Name(String),
    Op(char),
}

fn lex(src: &str) -> Result<Vec<Tok>, ExprError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() || c == '$' {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || matches!(chars[i], '.' | ',')) {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            out.push(Tok::Num(Ratio::parse_decimal(&text)?));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Name(chars[start..i].iter().collect()));
        } else if "+-*/%()×÷".contains(c) {
            out.push(Tok::Op(match c {
                '×' => '*',
                '÷' => '/',
                c => c,
            }));
            i += 1;
        } else {
            return Err(ExprError::new(format!("unexpected \"{c}\"")));
        }
    }
    Ok(out)
}

struct Parser<'a, L: Lookup> {
    toks: Vec<Tok>,
    at: usize,
    vars: &'a L,
}

impl<L: Lookup> Parser<'_, L> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn eat(&mut self, op: char) -> bool {
        if self.peek() == Some(&Tok::Op(op)) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Result<Ratio, ExprError> {
        let mut v = self.term()?;
        loop {
            if self.eat('+') {
                v = v.checked_add(self.term()?)?;
            } else if self.eat('-') {
                v = v.checked_sub(self.term()?)?;
            } else {
                return Ok(v);
            }
        }
    }

    fn term(&mut self) -> Result<Ratio, ExprError> {
        let mut v = self.unary()?;
        loop {
            if self.eat('*') {
                v = v.checked_mul(self.unary()?)?;
            } else if self.eat('/') {
                v = v.checked_div(self.unary()?)?;
            } else {
                return Ok(v);
            }
        }
    }

    fn unary(&mut self) -> Result<Ratio, ExprError> {
        if self.eat('-') {
            return Ratio::ZERO.checked_sub(self.unary()?);
        }
        if self.eat('+') {
            return self.unary();
        }
        let mut v = self.primary()?;
        while self.eat('%') {
            v = v.checked_div(Ratio::int(100))?;
        }
        Ok(v)
    }

    fn primary(&mut self) -> Result<Ratio, ExprError> {
        match self.toks.get(self.at).cloned() {
            Some(Tok::Num(r)) => {
                self.at += 1;
                Ok(r)
            }
            Some(Tok::Name(n)) => {
                self.at += 1;
                self.vars.number(&n)
            }
            Some(Tok::Op('(')) => {
                self.at += 1;
                let v = self.expr()?;
                if !self.eat(')') {
                    return Err(ExprError::new("a \"(\" is never closed"));
                }
                Ok(v)
            }
            Some(Tok::Op(c)) => Err(ExprError::new(format!("unexpected \"{c}\""))),
            None => Err(ExprError::new("the expression ends too soon")),
        }
    }
}

/// Evaluate an expression exactly.
pub fn eval(src: &str, vars: &impl Lookup) -> Result<Ratio, ExprError> {
    let toks = lex(src)?;
    if toks.is_empty() {
        return Err(ExprError::new("nothing to work out"));
    }
    let mut p = Parser { toks, at: 0, vars };
    let v = p.expr()?;
    match p.peek() {
        None => Ok(v),
        Some(Tok::Op(')')) => Err(ExprError::new("a \")\" has no matching \"(\"")),
        Some(_) => Err(ExprError::new("two values with no operator between them")),
    }
}

/// Evaluate an expression and round it to the cent.
pub fn eval_money(src: &str, vars: &impl Lookup) -> Result<Money, ExprError> {
    eval(src, vars)?.to_money()
}

/// Whether `src` is a bare amount rather than a formula - "12.50", not
/// "25 / 2". Front ends show the worked-out value only for formulas.
pub fn is_plain_amount(src: &str) -> bool {
    Money::parse(src.trim()).is_ok()
}

/// Names used in an expression, in order of first use. For listing which
/// variables an entry depends on.
pub fn names(src: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in lex(src).unwrap_or_default() {
        if let Tok::Name(n) = t {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(&n)) {
                out.push(n);
            }
        }
    }
    out
}

#[cfg(test)]
// Cents written as dollars_cents: `136_00` is $136.00.
#[allow(clippy::inconsistent_digit_grouping)]
mod tests {
    use super::*;

    struct Vars;
    impl Lookup for Vars {
        fn number(&self, name: &str) -> Result<Ratio, ExprError> {
            match name.to_lowercase().as_str() {
                "car_km_rate" => Ratio::parse_decimal("0.68"),
                "apr" => Ratio::parse_decimal("6.45"),
                _ => Err(ExprError::new(format!("no variable called {name}"))),
            }
        }
    }

    fn m(src: &str) -> Money {
        eval_money(src, &Vars).unwrap_or_else(|e| panic!("{src}: {e}"))
    }

    #[test]
    fn works_out_what_people_type() {
        assert_eq!(m("200*Car_Km_Rate"), Money(136_00));
        assert_eq!(m("200 * car_km_rate"), Money(136_00), "names ignore case");
        assert_eq!(m("12.50"), Money(12_50));
        assert_eq!(m("$1,200.00 / 4"), Money(300_00));
        assert_eq!(m("(1850 - 400) / 2"), Money(725_00));
        assert_eq!(m("-3 + 5"), Money(2_00));
        assert_eq!(m("2 * -3"), Money(-6_00));
        assert_eq!(m("5% * 1000"), Money(50_00));
        assert_eq!(m("1000 * apr% / 12"), Money(5_38), "5.375 rounds up");
        assert_eq!(m("100 × 3 ÷ 4"), Money(75_00));
    }

    #[test]
    fn is_exact_until_the_last_step() {
        assert_eq!(m("0.1 + 0.2"), Money(30));
        assert_eq!(m("100 / 3 * 3"), Money(100_00));
        assert_eq!(m("10 / 3"), Money(3_33));
        assert_eq!(m("20 / 3"), Money(6_67));
        assert_eq!(m("0.005"), Money(1), "half a cent rounds away from zero");
        assert_eq!(m("-0.005"), Money(-1));
        assert_eq!(m("0.0049"), Money(0));
    }

    #[test]
    fn says_what_is_wrong() {
        for (src, says) in [
            ("", "nothing"),
            ("2 +", "ends too soon"),
            ("(2 + 3", "never closed"),
            ("2 + 3)", "no matching"),
            ("2 3", "no operator"),
            ("1 / 0", "division by zero"),
            ("2 * Nope", "no variable called Nope"),
            ("2 ^ 3", "unexpected"),
            ("1.2.3", "not a number"),
            ("99999999999999999999999999999999999999 * 10", "too large"),
        ] {
            let err = eval(src, &Vars).unwrap_err().to_string();
            assert!(err.contains(says), "{src:?} gave {err:?}");
        }
    }

    #[test]
    fn prints_as_a_decimal_when_it_can() {
        let r = |s: &str| eval(s, &()).unwrap().to_string();
        assert_eq!(r("0.68"), "0.68");
        assert_eq!(r("6.45 / 100"), "0.0645");
        assert_eq!(r("3"), "3");
        assert_eq!(r("-1/8"), "-0.125");
        assert_eq!(r("1/3"), "1/3");
    }

    #[test]
    fn finds_the_names_used() {
        assert_eq!(names("200 * Car_Km_Rate + car_km_rate - Toll"), ["Car_Km_Rate", "Toll"]);
        assert!(is_plain_amount("1,200.50"));
        assert!(!is_plain_amount("25 / 2"));
    }
}
