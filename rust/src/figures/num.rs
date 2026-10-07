//! The three kinds of number Rails' figures are made of, with Ruby's own coercion: an Integer stays an Integer
//! until it meets a BigDecimal or a Float, a Float that meets a BigDecimal becomes one (Float#to_d), and
//! `[a, b].min` hands back one of its operands, type and all. The kind is part of the figure: Rails prints an
//! Integer zero as `0`, a BigDecimal zero as `"0.0"` and a Float zero as `0.0`.
//! Every rule here is pinned by vectors recorded from Ruby (script/rust/record_figures_vectors.rb).
use super::dec::Dec;
use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq)]
pub enum Num { Int(i64), Dec(Dec), Float(f64) }

/// Why a number could not be made.
#[derive(Clone, Debug, PartialEq)]
pub enum NumError {
    /// Ruby raises here too (`ZeroDivisionError: divided by 0`), as `"<class>: <message>"`.
    Raised(String),
    /// A number this library does not hold: an Integer beyond 64 bits, or a decimal beyond the limits of `dec`.
    /// Ruby carries such a number on; here the figure is not computed.
    OutOfRange,
    /// The figure's arithmetic passed its budget (`budget`). Ruby computes on, for as long as it takes.
    OverBudget,
    /// Infinity or NaN, or a text that is no clean decimal. Ruby carries the first two on (and prints them as
    /// `null`) and reads a number off the front of the third; here the figure is not computed.
    NotANumber,
}

fn zero_division() -> NumError { NumError::Raised("ZeroDivisionError: divided by 0".into()) }
fn finite(f: f64) -> Result<Num, NumError> { if f.is_finite() { Ok(Num::Float(f)) } else { Err(NumError::NotANumber) } }

impl Num {
    pub fn zero() -> Num { Num::Int(0) }

    /// A JSON number as Ruby's parser reads it: an Integer or a Float. None for a value that is not a number.
    /// An integer token within 64 signed bits is an Integer. One beyond them is refused (Ruby carries it on exactly):
    /// serde_json holds it as a u64 or, beyond 64 unsigned bits, has already made a Float of it, so any number of 2^63
    /// or more with no fraction is refused: the token and such a Float cannot be told apart here, and neither is a
    /// rate or a price. A Float is a provider's number, and is held to
    /// the venue's limit: zero, or its first digit within 10^±40.
    pub fn from_json(v: &serde_json::Value) -> Result<Option<Num>, NumError> {
        let Some(n) = v.as_number() else { return Ok(None) };
        if let Some(i) = n.as_i64() { return Ok(Some(Num::Int(i))); }
        let whole_and_wide = |f: f64| f.fract() == 0.0 && f.abs() >= 9_223_372_036_854_775_808.0;
        match n.as_f64() {
            Some(f) if !whole_and_wide(f) && (f == 0.0 || (1e-40..1e41).contains(&f.abs())) => Ok(Some(Num::Float(f))),
            _ => Err(NumError::OutOfRange),
        }
    }

    /// Provider fiat operands use Ruby Float, accepting only whole trimmed decimal strings.
    /// Unlike String#to_f, a numeric prefix followed by garbage is never accepted.
    pub fn from_fx_json(value: &serde_json::Value) -> Result<Option<Num>, NumError> {
        let number = match value {
            serde_json::Value::String(text) => text.trim().parse::<f64>().map_err(|_| NumError::NotANumber)?,
            // Preserve the existing JSON-number domain guard; totals converts these to f64.
            _ => return Self::from_json(value),
        };
        finite(number).map(Some)
    }

    /// `to_d`: Integer exactly, Float through its shortest digits (Float#to_d).
    pub fn to_d(&self) -> Result<Dec, NumError> {
        match self {
            Num::Int(i) => Ok(Dec::from_i64(*i)),
            Num::Dec(d) => Ok(d.clone()),
            Num::Float(f) => Dec::from_f64(*f),
        }
    }
    pub fn to_f(&self) -> f64 {
        match self { Num::Int(i) => *i as f64, Num::Dec(d) => d.to_f(), Num::Float(f) => *f }
    }
    pub fn is_zero(&self) -> bool {
        match self { Num::Int(i) => *i == 0, Num::Dec(d) => d.is_zero(), Num::Float(f) => *f == 0.0 }
    }
    pub fn is_positive(&self) -> bool {
        match self { Num::Int(i) => *i > 0, Num::Dec(d) => d.is_positive(), Num::Float(f) => *f > 0.0 }
    }
    pub fn is_negative(&self) -> bool {
        match self { Num::Int(i) => *i < 0, Num::Dec(d) => d.is_negative(), Num::Float(f) => *f < 0.0 }
    }

    fn arith(&self, o: &Num, int: fn(i64, i64) -> Option<i64>, dec: fn(&Dec, &Dec) -> Result<Dec, NumError>, float: fn(f64, f64) -> f64) -> Result<Num, NumError> {
        match (self, o) {
            (Num::Int(a), Num::Int(b)) => Ok(Num::Int(int(*a, *b).ok_or(NumError::OutOfRange)?)),
            (Num::Float(a), Num::Float(b)) => finite(float(*a, *b)),
            (Num::Float(a), Num::Int(b)) => finite(float(*a, *b as f64)),
            (Num::Int(a), Num::Float(b)) => finite(float(*a as f64, *b)),
            _ => Ok(Num::Dec(dec(&self.to_d()?, &o.to_d()?)?)),
        }
    }
    pub fn add(&self, o: &Num) -> Result<Num, NumError> { self.arith(o, i64::checked_add, |a, b| a + b, |a, b| a + b) }
    pub fn sub(&self, o: &Num) -> Result<Num, NumError> { self.arith(o, i64::checked_sub, |a, b| a - b, |a, b| a - b) }
    pub fn mul(&self, o: &Num) -> Result<Num, NumError> { self.arith(o, i64::checked_mul, |a, b| a * b, |a, b| a * b) }
    /// Integer / Integer floors, as Ruby's does, and a BigDecimal or an Integer divided by zero raises.
    pub fn div(&self, o: &Num) -> Result<Num, NumError> {
        match (self, o) {
            (Num::Int(_), Num::Int(0)) => Err(zero_division()),
            (Num::Int(a), Num::Int(b)) => Ok(Num::Int(a.checked_div_euclid(*b).map(|q| if *b < 0 && a.rem_euclid(*b) != 0 { q - 1 } else { q }).ok_or(NumError::OutOfRange)?)),
            (Num::Float(a), Num::Float(b)) => finite(a / b),
            (Num::Float(a), Num::Int(b)) => finite(a / *b as f64),
            (Num::Int(a), Num::Float(b)) => finite(*a as f64 / b),
            _ => Ok(Num::Dec(self.to_d()?.div(&o.to_d()?)?)),
        }
    }

    /// `<=>`. Anything against a BigDecimal is compared as BigDecimals.
    pub fn compare(&self, o: &Num) -> Result<Ordering, NumError> {
        match (self, o) {
            (Num::Int(a), Num::Int(b)) => Ok(a.cmp(b)),
            (Num::Float(a), Num::Float(b)) => a.partial_cmp(b).ok_or(NumError::NotANumber),
            // ponytail: through f64, which is exact for an Integer below 2^53; no figure compares a larger one with a Float.
            (Num::Int(a), Num::Float(b)) => (*a as f64).partial_cmp(b).ok_or(NumError::NotANumber),
            (Num::Float(a), Num::Int(b)) => a.partial_cmp(&(*b as f64)).ok_or(NumError::NotANumber),
            _ => Ok(self.to_d()?.cmp(&o.to_d()?)),
        }
    }
    pub fn lt(&self, o: &Num) -> Result<bool, NumError> { Ok(self.compare(o)? == Ordering::Less) }
    pub fn gt(&self, o: &Num) -> Result<bool, NumError> { Ok(self.compare(o)? == Ordering::Greater) }

    /// `[a, b].min`: the second only when it is strictly smaller, so a tie keeps the first operand and its kind.
    pub fn min2(a: Num, b: Num) -> Result<Num, NumError> { Ok(if b.lt(&a)? { b } else { a }) }
    /// `[a, b].max`: the second only when it is strictly greater.
    pub fn max2(a: Num, b: Num) -> Result<Num, NumError> { Ok(if b.gt(&a)? { b } else { a }) }

    /// `round(places)`, for the places `dec` rounds to. A BigDecimal rounds half away from zero, and with no places
    /// left it becomes an Integer (BigDecimal#round(0)); an Integer is itself.
    /// ponytail: Float#round is not ported; no figure rounds a Float.
    pub fn round(&self, places: i64) -> Result<Num, NumError> {
        match self {
            Num::Dec(d) if places == 0 => {
                let whole = d.round(0)?;
                if whole.is_zero() { return Ok(Num::Int(0)); }
                whole.to_s_f().trim_end_matches(".0").parse().map(Num::Int).map_err(|_| NumError::OutOfRange)
            }
            Num::Dec(d) => Ok(Num::Dec(d.round(places)?)),
            other => Ok(other.clone()),
        }
    }
}

/// The shortest digits that read back as `f`, and the position of the decimal point after the first of them.
pub(super) fn shortest(f: f64) -> (bool, String, i32) {
    let sci = format!("{:e}", f.abs()); // "1.2345e-5", "1e16"
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    (f.is_sign_negative(), mantissa.chars().filter(char::is_ascii_digit).collect(), exp.parse().unwrap_or(0))
}

/// Float#to_s: plain from 1e-4 up to (not including) 1e15, else `d.ddde+XX`; always a fractional digit.
pub fn float_to_s(f: f64) -> String {
    if f.is_nan() { return "NaN".into(); }
    if f.is_infinite() { return if f > 0.0 { "Infinity".into() } else { "-Infinity".into() }; }
    if f == 0.0 { return if f.is_sign_negative() { "-0.0".into() } else { "0.0".into() }; }
    let (negative, digits, exp) = shortest(f);
    let decpt = exp + 1; // digits before the point
    let n = digits.len() as i32;
    // Sixteen digits before the point are still written out when a seventeenth follows it.
    let body = if 0 < decpt && (decpt <= 15 || (decpt == 16 && n > 16)) {
        if n <= decpt { format!("{digits}{}.0", "0".repeat((decpt - n) as usize)) }
        else { format!("{}.{}", &digits[..decpt as usize], &digits[decpt as usize..]) }
    } else if -4 < decpt && decpt <= 0 {
        format!("0.{}{digits}", "0".repeat((-decpt) as usize))
    } else {
        let rest = if n > 1 { &digits[1..] } else { "0" };
        format!("{}.{rest}e{}{:02}", &digits[..1], if exp < 0 { '-' } else { '+' }, exp.abs())
    };
    if negative { format!("-{body}") } else { body }
}

/// C's `%0.16g`: sixteen significant digits, trailing zeros dropped, an exponent below -4 or from 16 up.
fn g16(f: f64) -> String {
    let sci = format!("{f:.15e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let trim = |s: &str| if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() };
    if !(-4..16).contains(&exp) {
        format!("{}e{}{:02}", trim(mantissa), if exp < 0 { '-' } else { '+' }, exp.abs())
    } else {
        trim(&format!("{f:.*}", (15 - exp) as usize))
    }
}

/// A Float as Rails writes it into JSON. Rails' encoder is Oj's (config/initializers/oj.rb), not the json gem:
/// a whole number prints with one decimal, anything else with sixteen significant digits (`%0.16g`), except
/// where that ends in 0001 or 9999, which falls back to Float#to_s. Sixteen digits do not always read back
/// as the same double: the page receives the number Rails printed, not the one it computed.
pub fn oj_float(f: f64) -> String {
    if f == 0.0 { return "0.0".into(); }
    if !f.is_finite() { return "null".into(); }
    if f == (f as i64) as f64 { return format!("{f:.1}"); }
    let s = g16(f);
    if s.len() >= 17 && (s.ends_with("0001") || s.ends_with("9999")) { float_to_s(f) } else { s }
}
