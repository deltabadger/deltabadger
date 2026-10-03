//! Ruby semantics that Rails' output depends on, each pinned by vectors recorded from Ruby
//! (script/rust/record_vectors.rb: "bigdec", "ruby"). BigDec is Ruby's BigDecimal as bigdecimal 3.3.1
//! computes it: exact + − ×, and division rounded half-up to a precision that depends on the operands.
use crate::codec::CodecError;
use bigdecimal::num_bigint::{BigInt, Sign};
use bigdecimal::{BigDecimal, RoundingMode};
use chrono::{DateTime, Utc};
use rusqlite::types::ValueRef;
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BigDec(BigDecimal);

fn digit_count(i: &BigInt) -> i64 { if i.sign() == Sign::NoSign { 1 } else { i.magnitude().to_string().len() as i64 } }

impl BigDec {
    pub fn parse(s: &str) -> Result<Self, CodecError> {
        BigDecimal::from_str(s.trim()).map(|d| Self(d.normalized())).map_err(|e| CodecError::Decimal(format!("{s:?}: {e}")))
    }
    pub fn from_i64(i: i64) -> Self { Self(BigDecimal::from(i)) }
    /// Float#to_d: the shortest round-trip digits, truncated (not rounded) to 16 significant digits.
    /// Built directly as digits x 10^exp, so there is no range limit besides f64's own.
    pub fn from_f64(f: f64) -> Result<Self, CodecError> {
        let err = || CodecError::Decimal(format!("{f:e} is not finite"));
        if !f.is_finite() { return Err(err()); }
        let sci = format!("{f:e}"); // e.g. "-1.2345678912345679e8": the shortest round-trip digits
        let (mantissa, exp) = sci.split_once('e').ok_or_else(err)?;
        let exp: i64 = exp.parse().map_err(|_| err())?;
        let digits: String = mantissa.chars().filter(char::is_ascii_digit).take(16).collect();
        let int: BigInt = digits.parse().map_err(|_| err())?;
        let int = if mantissa.starts_with('-') { -int } else { int };
        Ok(Self(BigDecimal::new(int, digits.len() as i64 - 1 - exp).normalized()))
    }
    pub fn zero() -> Self { Self::from_i64(0) }
    pub fn one() -> Self { Self::from_i64(1) }
    pub fn is_zero(&self) -> bool { bigdecimal::Zero::is_zero(&self.0) }
    pub fn is_positive(&self) -> bool { self.0.sign() == bigdecimal::num_bigint::Sign::Plus }
    pub fn max(self, other: Self) -> Self { if other > self { other } else { self } }

    /// BigDecimal#precision: significant digits, counting a pure fraction's leading zeros.
    pub fn precision(&self) -> i64 {
        let (int, scale) = self.0.normalized().as_bigint_and_exponent();
        let n = digit_count(&int);
        if scale <= 0 { n - scale } else { n.max(scale) }
    }

    /// BigDecimal#/ (BigDecimal_div2 with n = 0). None for a zero divisor, where Ruby gives Infinity.
    pub fn div(&self, other: &Self) -> Option<Self> {
        if other.is_zero() { return None; }
        if self.is_zero() { return Some(Self::zero()); }
        let ix = (self.precision().max(other.precision()) + 16).max(32);
        let (ia, sa) = self.0.normalized().as_bigint_and_exponent();
        let (ib, sb) = other.0.normalized().as_bigint_and_exponent();
        let negative = (ia.sign() == Sign::Minus) != (ib.sign() == Sign::Minus);
        let (ma, mb) = (BigInt::from(ia.magnitude().clone()), BigInt::from(ib.magnitude().clone()));
        // Enough digits for ix + 1 significant digits of the floor quotient.
        let k = (ix + 2 + digit_count(&mb) - digit_count(&ma)).max(0);
        let q0 = (&ma * BigInt::from(10u8).pow(k as u32)) / &mb;
        let mut exponent = sa - sb + k; // value = q0 × 10^-exponent
        let d = digit_count(&q0);
        let q = if d > ix {
            let drop = (d - ix) as u32;
            let unit = BigInt::from(10u8).pow(drop);
            let (mut q1, rest) = (&q0 / &unit, &q0 % &unit);
            if rest * 2 >= unit { q1 += 1; } // ROUND_HALF_UP; the floor only ever loses digits below
            exponent -= drop as i64;
            q1
        } else { q0 };
        Some(Self(BigDecimal::new(if negative { -q } else { q }, exponent).normalized()))
    }

    fn scaled(&self, places: i64, mode: RoundingMode) -> Self { Self(self.0.with_scale_round(places, mode).normalized()) }
    pub fn floor(&self, places: i64) -> Self { self.scaled(places, RoundingMode::Floor) }
    pub fn ceil(&self, places: i64) -> Self { self.scaled(places, RoundingMode::Ceiling) }
    pub fn round(&self, places: i64) -> Self { self.scaled(places, RoundingMode::HalfUp) }

    /// BigDecimal#to_s('F'): plain notation, at least one fractional digit.
    pub fn to_s_f(&self) -> String {
        let n = self.0.normalized();
        let s = if n.fractional_digit_count() <= 0 { n.with_scale(0).to_string() } else { n.to_plain_string() };
        let s = if s == "-0" { "0".to_string() } else { s };
        if s.contains('.') { s } else { format!("{s}.0") }
    }
    /// BigDecimal#to_f: the correctly rounded double.
    pub fn to_f(&self) -> f64 { f64::from_str(&self.to_s_f()).expect("plain decimal parses") }
}

impl std::ops::Add for &BigDec { type Output = BigDec; fn add(self, o: &BigDec) -> BigDec { BigDec((&self.0 + &o.0).normalized()) } }
impl std::ops::Sub for &BigDec { type Output = BigDec; fn sub(self, o: &BigDec) -> BigDec { BigDec((&self.0 - &o.0).normalized()) } }
impl std::ops::Mul for &BigDec { type Output = BigDec; fn mul(self, o: &BigDec) -> BigDec { BigDec((&self.0 * &o.0).normalized()) } }

/// How ActiveRecord reads a decimal column from SQLite (NUMERIC affinity: INTEGER, REAL or TEXT).
pub fn from_sql(v: ValueRef<'_>) -> Result<Option<BigDec>, CodecError> {
    match v {
        ValueRef::Null => Ok(None),
        ValueRef::Integer(i) => Ok(Some(BigDec::from_i64(i))),
        ValueRef::Real(f) => BigDec::from_f64(f).map(Some),
        ValueRef::Text(t) => BigDec::parse(std::str::from_utf8(t).map_err(|e| CodecError::Decimal(e.to_string()))?).map(Some),
        ValueRef::Blob(_) => Err(CodecError::Decimal("blob in a decimal column".into())),
    }
}

/// How Rails writes one: Transaction#before_save's round(18), then the SQLite adapter binds BigDecimal#to_f.
pub fn to_sql(d: &BigDec) -> f64 { d.round(18).to_f() }

/// `Time#as_json` for a UTC time: ISO 8601 with milliseconds and `Z`.
pub fn iso8601_ms(t: DateTime<Utc>) -> String { t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string() }

/// Exact `(Time.at(anchor_us µs) + Σ kᵢ·fᵢ).round(6)` in µs. `Time + Float` adds the float's exact binary
/// value, so k additions of f add exactly k·f; `Time#round(6)` rounds half up. |kᵢ·fᵢ| < 2^40 s.
pub fn round6_micros(anchor_us: i64, terms: &[(f64, i64)]) -> i64 {
    let parts: Vec<(i128, i32)> = terms.iter().map(|&(f, k)| { let (m, e) = decompose(f); (m * i128::from(k), e) }).collect();
    let e_min = parts.iter().map(|&(_, e)| e).min().unwrap_or(0).min(0);
    let sum: i128 = parts.iter().map(|&(m, e)| m << (e - e_min)).sum();
    let scaled = sum * 1_000_000;
    let shift = (-e_min) as u32;
    let micros = if shift == 0 { scaled } else {
        let (q, r) = (scaled.div_euclid(1i128 << shift), scaled.rem_euclid(1i128 << shift));
        if r >= (1i128 << (shift - 1)) { q + 1 } else { q }
    };
    anchor_us + micros as i64
}

/// Exact `Time.at(anchor_us) + k·f > Time.at(now_us)`.
pub fn exceeds(anchor_us: i64, (f, k): (f64, i64), now_us: i64) -> bool {
    let (m, e) = decompose(f);
    let lhs = m * i128::from(k) * 1_000_000; // offset in µs × 2^-e
    let diff = i128::from(now_us - anchor_us);
    if e >= 0 { (lhs << e) > diff } else { lhs > (diff << (-e) as u32) }
}

fn decompose(f: f64) -> (i128, i32) {
    if f == 0.0 { return (0, 0); }
    let bits = f.to_bits();
    let sign: i128 = if bits >> 63 == 1 { -1 } else { 1 };
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let frac = (bits & ((1u64 << 52) - 1)) as i128;
    let (m, e) = if exp == 0 { (frac, -1074) } else { (frac | (1i128 << 52), exp - 1075) };
    (sign * m, e)
}

/// `Array#to_sentence` (ActiveSupport, English).
pub fn to_sentence(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        [init @ .., last] => format!("{}, and {last}", init.join(", ")),
    }
}

/// `Array#inspect` for strings: `["a", "b \"c\""]`.
pub fn inspect(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>().join(", "))
}

/// Array#sum over Floats (array.c ary_sum): Kahan-Babuska compensated summation from Integer 0. Not a left fold:
/// [0.1, 0.2, 0.3].sum is 0.6 in Ruby. Pinned by the float_sum vectors.
pub fn float_sum(xs: &[f64]) -> f64 {
    let (mut f, mut c) = (0.0f64, 0.0f64);
    for &x in xs {
        let t = f + x;
        if f.abs() >= x.abs() { c += (f - t) + x; } else { c += (x - t) + f; }
        f = t;
    }
    f + c
}

/// Float#round(ndigits) for 0 < ndigits <= 14 (float.c rb_float_round with round_half_up): `round(x * 10**n)`, raised by one
/// when the halfway point divided back still lies at or below x, then divided by 10**n. A value already exact at this many
/// digits (float_round_overflow) is returned unchanged, and a positive one too small for them (float_round_underflow) is 0.
pub fn float_round(x: f64, ndigits: i32) -> f64 {
    assert!((1..=14).contains(&ndigits), "Float#round({ndigits}) takes Ruby's rational path, not ported");
    if x == 0.0 || !x.is_finite() || x.is_subnormal() { return x; }
    let binexp = (((x.to_bits() >> 52) & 0x7ff) as i32) - 1022; // frexp's exponent: x = m × 2^binexp, 0.5 <= |m| < 1
    if ndigits >= 17 - if binexp > 0 { binexp / 4 } else { binexp / 3 - 1 } { return x; }
    if x > 0.0 && ndigits < -(if binexp > 0 { binexp / 3 + 1 } else { binexp / 4 }) { return 0.0; }
    let s = 10f64.powi(ndigits);
    let mut f = (x * s).round();
    if x > 0.0 { if (f + 0.5) / s <= x { f += 1.0; } } else if (f - 0.5) / s >= x { f -= 1.0; }
    f / s
}

/// ActiveModel::Type::Decimal#cast_value for a Float, on a column with precision and scale such as decimal(10,6):
/// `BigDecimal(value.round(scale), float_precision)` (dtoa's correctly rounded significant digits, at most Float::DIG + 1),
/// then `round(scale)` again, half up. The same cast reads a REAL back from SQLite. Pinned by the decimal_10_6 vectors.
pub fn decimal_column(f: f64, precision: usize, scale: i32) -> Result<BigDec, CodecError> {
    let rounded = float_round(f, scale);
    if !rounded.is_finite() { return Err(CodecError::Decimal(format!("{f:e} is not finite"))); }
    Ok(BigDec::parse(&format!("{:.*e}", precision.clamp(1, 16) - 1, rounded))?.round(scale as i64))
}

/// A Ruby numeric as Rails' amount-cap arithmetic carries it: Integer, Float or BigDecimal.
/// Bot::QuoteAmountLimitable mixes all three: decimal columns pluck as BigDecimal, `pluck(Arel.sql('COALESCE(...)'))` as
/// SQLite's own INTEGER or REAL, and the settings JSON holds the limit as an Integer or a Float. Pinned by the amount_caps
/// vectors.
#[derive(Clone, Debug, PartialEq)]
pub enum Num { Int(i64), Float(f64), Dec(BigDec) }

impl Num {
    /// self + o, or self - o with `sub`, coerced as Ruby does: Integer with Integer stays Integer; Integer with Float is
    /// Float arithmetic; anything with a BigDecimal is BigDecimal arithmetic, a Float operand converted as BigDecimal#coerce
    /// converts it (Float#to_d: 16 significant digits, BigDec::from_f64).
    fn op(&self, o: &Num, sub: bool) -> Result<Num, CodecError> {
        Ok(match (self, o) {
            (Num::Int(a), Num::Int(b)) => Num::Int(if sub { a - b } else { a + b }),
            (Num::Int(_) | Num::Float(_), Num::Int(_) | Num::Float(_)) => {
                let (a, b) = (self.to_f(), o.to_f());
                Num::Float(if sub { a - b } else { a + b })
            }
            _ => {
                let (a, b) = (self.to_dec()?, o.to_dec()?);
                Num::Dec(if sub { &a - &b } else { &a + &b })
            }
        })
    }
    pub fn add(&self, o: &Num) -> Result<Num, CodecError> { self.op(o, false) }
    pub fn sub(&self, o: &Num) -> Result<Num, CodecError> { self.op(o, true) }
    fn to_f(&self) -> f64 { match self { Num::Int(i) => *i as f64, Num::Float(f) => *f, Num::Dec(d) => d.to_f() } }
    pub fn is_negative(&self) -> bool { match self { Num::Int(i) => *i < 0, Num::Float(f) => *f < 0.0, Num::Dec(d) => *d < BigDec::zero() } }
    /// The value as a BigDecimal, a Float as Float#to_d (what a later BigDecimal operation in Ruby makes of it).
    pub fn to_dec(&self) -> Result<BigDec, CodecError> {
        match self { Num::Int(i) => Ok(BigDec::from_i64(*i)), Num::Float(f) => BigDec::from_f64(*f), Num::Dec(d) => Ok(d.clone()) }
    }
    /// self < f, as Ruby compares with a Float: a BigDecimal against Float#to_d, the others as doubles.
    pub fn lt_f64(&self, f: f64) -> Result<bool, CodecError> {
        Ok(match self { Num::Dec(d) => *d < BigDec::from_f64(f)?, _ => self.to_f() < f })
    }
}

/// Array#sum (array.c ary_sum) over Integers, Floats and BigDecimals: Integers add exactly; from the first Float on, a
/// Kahan-Babuska sum in doubles that starts from the Integer prefix (later Integers join as doubles); anything else leaves
/// that path (the double so far, uncompensated, as Ruby's `not_float` does) and is added with `+`. Empty is Integer 0.
pub fn ruby_sum(xs: &[Num]) -> Result<Num, CodecError> {
    let mut i = 0;
    let mut n: i64 = 0;
    while let Some(Num::Int(x)) = xs.get(i) { n += x; i += 1; }
    let mut v = Num::Int(n);
    if let Some(Num::Float(_)) = xs.get(i) {
        let (mut f, mut c) = (n as f64, 0.0f64);
        let mut left = false;
        while let Some(e) = xs.get(i) {
            let x = match e { Num::Float(x) => *x, Num::Int(x) => *x as f64, Num::Dec(_) => { left = true; break } };
            let t = f + x;
            if f.abs() >= x.abs() { c += (f - t) + x; } else { c += (x - t) + f; }
            f = t;
            i += 1;
        }
        v = Num::Float(if left { f } else { f + c });
    }
    for e in &xs[i..] { v = v.add(e)?; }
    Ok(v)
}
