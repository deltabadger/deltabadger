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
