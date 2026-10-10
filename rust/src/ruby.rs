//! Ruby semantics that Rails' output depends on, each pinned by vectors recorded from Ruby
//! (script/rust/record_vectors.rb: "bigdec", "ruby"). BigDec is Ruby's BigDecimal as bigdecimal 4.1.3
//! computes it: exact + − ×, and division rounded half-up to a precision that depends on the operands.
use crate::codec::CodecError;
use bigdecimal::num_bigint::{BigInt, Sign};
use bigdecimal::{BigDecimal, RoundingMode};
use chrono::{DateTime, Utc};
use rusqlite::types::ValueRef;
use serde_json::Value;
use std::str::FromStr;

/// The longest text `parse` reads. Real decimals are under 50 characters (a venue's price or fill, a value rounded to 18
/// places, a 32-to-50-digit quotient written into a placement intent); 256 leaves room and bounds the parse itself.
pub const MAX_INPUT_LEN: usize = 256;
/// The most significant digit must lie within 10^-400 ..= 10^400. Money needs 10^-18 ..= 10^15; every finite f64
/// (4.9e-324 ..= 1.8e308), so Float#to_d of any REAL column, fits with room.
pub const MAX_EXPONENT: i64 = 400;
/// At most 512 digits written out in full (BigDecimal#precision: a pure fraction's leading zeros count). The widest f64
/// needs 339 (16 digits at 10^-324); a value this wide costs a few hundred bytes to write out, compare or divide.
pub const MAX_DIGITS: i64 = 512;
/// Venue numbers (`json_to_d`: every price, quantity, fill and balance taken from a venue response) are held tighter:
/// the most significant digit within 10^±40 and at most 64 significant digits. Real values are far inside: Alpaca crypto
/// prices run ~1e-8 (SHIB-class) to ~1e6 with 9-decimal quantities and notionals to ~1e7; Kraken prices run ~1e-10
/// (BTC-quoted pairs) to ~1e6 with 8-10 decimal volumes and meme-coin balances to ~1e12; none has more than ~25
/// significant digits. Inside these caps every product or quotient of two venue numbers stays inside f64 (no ±Inf is
/// ever stored) and writes out in well under MAX_INPUT_LEN characters.
pub const VENUE_MAX_EXPONENT: i64 = 40;
pub const VENUE_MAX_DIGITS: i64 = 64;
/// The places a ticker's precision may round to (base, quote and price decimals). Venues use 0 ..= 18 (18 for wei);
/// 40 is headroom. Eligibility refuses a ticker outside it, and `scale` is the only way an engine precision becomes one.
pub const MAX_SCALE: i64 = 40;

/// Every BigDec holds at most MAX_DIGITS digits within 10^±MAX_EXPONENT when it is built from outside data (`parse`,
/// `from_f64`, `from_sql`, `json_to_d`); `from_i64` always fits. The bigdecimal crate stores a value as digits × 10^-scale,
/// so "1e-1000000000" is two small numbers until something writes it out, aligns it or divides by it: the bounds are
/// checked at construction, where refusing costs nothing.
///
/// The fixed-expression operators are not re-checked; each is bounded by its operands (n = digits written out):
/// `+` and `−`: n ≤ max(nₐ, n_b) + 1. `×`: n ≤ nₐ + n_b. `div`: at most max(precision) + 17 significant digits, and
/// its working integer is at most ~2·MAX_DIGITS + 20 digits. `floor`/`ceil`/`round`: n ≤ nₐ + 1 — a value already
/// within `places` is returned as it is, so `places` (a u8) never pads. Comparison aligns at most the operands' scales.
/// Fixed expressions of depth ≤ 4 over bounded inputs stay within a few thousand digits.
/// Accumulating ledger/split walks must instead use checked_add/checked_mul, which reapply the
/// construction bounds and charge the existing figures budget before each operation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BigDec(BigDecimal);

fn digit_count(i: &BigInt) -> i64 { if i.sign() == Sign::NoSign { 1 } else { i.magnitude().to_string().len() as i64 } }

impl BigDec {
    /// A decimal string within the bounds above; anything longer, wider or outside the exponent range is an error.
    pub fn parse(s: &str) -> Result<Self, CodecError> {
        let err = |e: String| CodecError::Decimal(format!("{s:?}: {e}"));
        if s.len() > MAX_INPUT_LEN { return Err(CodecError::Decimal(format!("a {}-character decimal (at most {MAX_INPUT_LEN})", s.len()))); }
        let t = s.trim();
        // The written exponent is read first, as a checked i64, so no exponent near i64's range reaches the parser's own
        // scale arithmetic (it computes digits − exponent in i64). Anything past ±(MAX_EXPONENT + MAX_INPUT_LEN) cannot pass
        // the check below anyway.
        if let Some(i) = t.find(['e', 'E']) {
            let limit = MAX_EXPONENT + MAX_INPUT_LEN as i64;
            if !t[i + 1..].parse::<i64>().is_ok_and(|e| (-limit..=limit).contains(&e)) { return Err(err("exponent out of range".into())); }
        }
        let d = BigDecimal::from_str(t).map_err(|e| err(e.to_string()))?;
        Self::bounded(d).map_err(err)
    }
    /// Checked before normalizing, in i128: whatever scale the parser produced, nothing here overflows or wraps.
    fn bounded(d: BigDecimal) -> Result<Self, String> {
        let (int, scale) = d.as_bigint_and_exponent();
        if int.sign() == Sign::NoSign { return Ok(Self::zero()); }
        let exponent = i128::from(digit_count(&int)) - 1 - i128::from(scale); // the most significant digit's power of ten
        if exponent.abs() > i128::from(MAX_EXPONENT) { return Err(format!("exponent {exponent} is outside ±{MAX_EXPONENT}")); }
        // Here |scale| ≤ digits + MAX_EXPONENT, so normalizing and `precision` are plain small-integer arithmetic.
        let v = Self(d.normalized());
        if v.precision() > MAX_DIGITS { return Err(format!("{} digits (at most {MAX_DIGITS})", v.precision())); }
        Ok(v)
    }
    /// Accumulating walks revalidate each operand/result and charge before arithmetic. Reuse
    /// figures' cumulative budget; fixed-depth engine expressions retain their existing operators.
    pub fn checked_mul(&self, other: &Self) -> Result<Self, CodecError> {
        let a = Self::bounded(self.0.clone()).map_err(CodecError::Decimal)?;
        let b = Self::bounded(other.0.clone()).map_err(CodecError::Decimal)?;
        let n = (a.precision().max(1) as u64).div_ceil(9);
        let m = (b.precision().max(1) as u64).div_ceil(9);
        crate::figures::budget::charge(n*m+n+m, n+m).map_err(|_| CodecError::Decimal("split walk arithmetic budget exceeded".into()))?;
        Self::bounded(&a.0 * &b.0).map_err(CodecError::Decimal)
    }
    pub fn checked_add(&self, other: &Self) -> Result<Self, CodecError> {
        let a = Self::bounded(self.0.clone()).map_err(CodecError::Decimal)?;
        let b = Self::bounded(other.0.clone()).map_err(CodecError::Decimal)?;
        // Alignment is bounded by both exponent ranges plus both precision caps.
        let limbs = ((2 * (MAX_DIGITS + MAX_EXPONENT)) as u64).div_ceil(9);
        crate::figures::budget::charge(limbs, limbs).map_err(|_| CodecError::Decimal("split walk arithmetic budget exceeded".into()))?;
        Self::bounded(&a.0 + &b.0).map_err(CodecError::Decimal)
    }

    /// The venue-number caps (VENUE_MAX_EXPONENT, VENUE_MAX_DIGITS) on a value already within BigDec's own.
    fn venue_bounded(self) -> Result<Self, CodecError> {
        if self.is_zero() { return Ok(self); }
        let (int, scale) = self.0.as_bigint_and_exponent();
        let (digits, exponent) = (digit_count(&int), digit_count(&int) - 1 - scale);
        if exponent.abs() > VENUE_MAX_EXPONENT || digits > VENUE_MAX_DIGITS {
            return Err(CodecError::Decimal(format!("{} is outside a venue number's range (10^±{VENUE_MAX_EXPONENT}, {VENUE_MAX_DIGITS} digits)", self.to_s_f())));
        }
        Ok(self)
    }
    /// `to_s_f`, only if it reads back through `parse` to this same value: what the engine may persist as a decimal
    /// string (a placement intent, the carry). Anything else is refused before it is written.
    pub fn to_persisted(&self) -> Result<String, CodecError> {
        let s = self.to_s_f();
        match Self::parse(&s) {
            Ok(back) if back == *self => Ok(s),
            _ => Err(CodecError::Decimal(format!("{}… ({} characters) would not read back", s.chars().take(24).collect::<String>(), s.len()))),
        }
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
        Self::bounded(BigDecimal::new(int, digits.len() as i64 - 1 - exp)).map_err(|e| CodecError::Decimal(format!("{f:e}: {e}")))
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

    /// A value with at most `places` fractional digits is already floored, ceiled and rounded (Ruby returns it unchanged), so
    /// it is returned as it is: with_scale_round would first pad it with zeros, `places` of them. `places` is a u8; an engine
    /// precision reaches it only through `scale`.
    fn scaled(&self, places: u8, mode: RoundingMode) -> Self {
        if self.0.fractional_digit_count() <= i64::from(places) { return self.clone(); }
        Self(self.0.with_scale_round(i64::from(places), mode).normalized())
    }
    pub fn floor(&self, places: u8) -> Self { self.scaled(places, RoundingMode::Floor) }
    pub fn ceil(&self, places: u8) -> Self { self.scaled(places, RoundingMode::Ceiling) }
    pub fn round(&self, places: u8) -> Self { self.scaled(places, RoundingMode::HalfUp) }
    /// `BigDecimal(x, digits)`: rounded half up to `digits` significant digits.
    pub fn round_sig(&self, digits: u64) -> Self {
        Self(self.0.with_precision_round(std::num::NonZeroU64::new(digits).expect("digits > 0"), RoundingMode::HalfUp).normalized())
    }

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

/// A precision from the database (tickers.base_decimals, quote_decimals, price_decimals: unbounded integers) as a rounding
/// scale: an error outside 0..=MAX_SCALE.
pub fn scale(places: i64) -> Result<u8, CodecError> {
    match u8::try_from(places) {
        Ok(p) if i64::from(p) <= MAX_SCALE => Ok(p),
        _ => Err(CodecError::Decimal(format!("{places} is outside 0..={MAX_SCALE}"))),
    }
}

/// A number in a venue's JSON as Ruby's `value.to_d` reads it: null is None (nil), an Integer exact, a Float by Float#to_d,
/// a String as a decimal, each within the venue caps (VENUE_MAX_*). DIVERGES from Ruby on purpose: String#to_d reads "garbage" as 0 ("12abc" as 12) and "NaN" or
/// "Infinity" as non-finite BigDecimals, and either would flow on as an ordinary price, quantity or fill. Here a string
/// that is not a finite decimal within the bounds above, and any other JSON type, is an error, never zero.
pub fn json_to_d(v: &Value) -> Result<Option<BigDec>, CodecError> {
    match v {
        Value::Null => Ok(None),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => BigDec::from_i64(i).venue_bounded().map(Some),
            (None, Some(f)) => BigDec::from_f64(f).and_then(BigDec::venue_bounded).map(Some),
            (None, None) => Err(CodecError::Decimal(format!("{n} is not a readable number"))),
        },
        Value::String(s) => BigDec::parse(s).and_then(BigDec::venue_bounded).map(Some),
        other => Err(CodecError::Decimal(format!("{} is not a number", raw(other)))),
    }
}

/// A venue value as an error message quotes it: a string bare, anything else as JSON, cut to 64 characters.
pub fn raw(v: &Value) -> String {
    let s = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
    if s.chars().count() > 64 { format!("{}…", s.chars().take(64).collect::<String>()) } else { s }
}

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

/// Fallible exact rounding for stored schedules, including subnormal/very large floats.
/// BigInt avoids intermediate shifts/products wrapping before the final i64 range check.
pub fn checked_round6_micros(anchor_us: i64, terms: &[(f64, i64)]) -> Option<i64> {
    use bigdecimal::num_traits::ToPrimitive;
    if terms.iter().any(|(f, _)| !f.is_finite()) { return None; }
    let parts: Vec<_> = terms.iter().map(|&(f, k)| {
        let (m, e) = decompose(f);
        (BigInt::from(m) * k, e)
    }).collect();
    let e_min = parts.iter().map(|(_, e)| *e).min().unwrap_or(0).min(0);
    let sum: BigInt = parts.iter().map(|(m, e)| m << (e - e_min) as usize).sum();
    let scaled = sum * 1_000_000;
    let divisor = BigInt::from(1) << (-e_min) as usize;
    // Floor((scaled / divisor) + 1/2), including negative times (Ruby half-up).
    let numerator: BigInt = scaled * 2 + &divisor;
    let denominator: BigInt = divisor * 2;
    let mut rounded: BigInt = &numerator / &denominator;
    if numerator.sign() == Sign::Minus && &numerator % &denominator != BigInt::from(0) { rounded -= 1; }
    (rounded + BigInt::from(anchor_us)).to_i64()
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
    Ok(BigDec::parse(&format!("{:.*e}", precision.clamp(1, 16) - 1, rounded))?.round(scale as u8)) // 1..=14: float_round asserted it
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
            (Num::Int(a), Num::Int(b)) => Num::Int(if sub { a.checked_sub(*b).ok_or(CodecError::IntegerOverflow("Num::sub"))? } else { a.checked_add(*b).ok_or(CodecError::IntegerOverflow("Num::add"))? }),
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
    while let Some(Num::Int(x)) = xs.get(i) { n = n.checked_add(*x).ok_or(CodecError::IntegerOverflow("ruby_sum"))?; i += 1; }
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

/// `Float#to_s` (numeric.c, flo_to_s): the shortest digits that read back as the same double, in plain notation while
/// the decimal point sits within 15 integer digits or 3 leading zeros, else `d.ddde±XX`. An integral value keeps `.0`.
pub fn float_to_s(f: f64) -> String {
    if f.is_nan() { return "NaN".into(); }
    if f.is_infinite() { return if f < 0.0 { "-Infinity".into() } else { "Infinity".into() }; }
    let sign = if f.is_sign_negative() { "-" } else { "" };
    if f == 0.0 { return format!("{sign}0.0"); }
    let sci = format!("{:e}", f.abs()); // "1.2345678912345679e8": the shortest round-trip digits
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let point = exp.parse::<i64>().unwrap_or(0) + 1; // digits before the decimal point (Ruby's decpt)
    let n = digits.len() as i64;
    if point > 15 || point <= -4 {
        let fraction = if n > 1 { &digits[1..] } else { "0" };
        return format!("{sign}{}.{fraction}e{}{:02}", &digits[..1], if point > 0 { '+' } else { '-' }, (point - 1).abs());
    }
    if point <= 0 { return format!("{sign}0.{}{digits}", "0".repeat((-point) as usize)); }
    if point >= n { return format!("{sign}{digits}{}.0", "0".repeat((point - n) as usize)); }
    format!("{sign}{}.{}", &digits[..point as usize], &digits[point as usize..])
}

/// `String#to_i`: leading whitespace, an optional sign, then digits (single underscores between digits allowed) up to
/// the first character that is none of these; 0 when there are none. Numbers past i64 saturate.
pub fn to_i(s: &str) -> i64 {
    let s = s.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let (negative, digits) = match s.as_bytes().first() { Some(b'-') => (true, &s[1..]), Some(b'+') => (false, &s[1..]), _ => (false, s) };
    let mut n: i64 = 0;
    let mut previous_digit = false;
    for b in digits.bytes() {
        match b {
            b'0'..=b'9' => { n = n.saturating_mul(10).saturating_add(i64::from(b - b'0')); previous_digit = true; }
            b'_' if previous_digit => previous_digit = false,
            _ => break,
        }
    }
    if negative { -n } else { n }
}

/// ActiveSupport's Time#as_json for `Time.parse(raw)`: xmlschema(3), fractions truncated to milliseconds, in the offset the
/// text gave ("Z" only when the text said Z: Time.parse makes that a UTC time; "+00:00" stays "+00:00"). Pinned by the
/// time_as_json vectors.
pub fn time_as_json(raw: &str, t: &chrono::DateTime<chrono::FixedOffset>) -> String {
    let base = t.format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
    if raw.ends_with(['Z', 'z']) { format!("{base}Z") } else { format!("{base}{}", t.format("%:z")) }
}


/// Ruby String#strip: ASCII whitespace plus NUL, never Unicode separators.
pub fn strip(value: &str) -> &str { value.trim_matches(|c| matches!(c, '\0'|' '|'\t'|'\n'|'\r'|'\x0b'|'\x0c')) }
/// ActiveSupport String#blank? uses the Unicode POSIX space class.
pub fn blank(value: &str) -> bool { value.chars().all(char::is_whitespace) }
/// Ruby \s is ASCII; keep Unicode letter classes enabled in the same expression.
pub fn validation_regex(pattern: &str) -> Result<regex::Regex, regex::Error> {
    regex::Regex::new(&pattern.replace(r"\s", r"[\x09-\x0d\x20]"))
}

#[cfg(test)]
mod settings_whitespace_tests {
    #[test]
    fn ruby_whitespace_separates_regexp_strip_and_blank_semantics(){
        let name=super::validation_regex(r"\A\p{L}+(\s+\p{L}+)*\z").unwrap();
        for separator in [" ","\t","\n","\r","\x0b","\x0c"]{assert!(name.is_match(&format!("Alice{separator}Smith")));}
        for separator in ["\u{a0}","\u{2003}"]{assert!(!name.is_match(&format!("Alice{separator}Smith")));assert_eq!(super::strip(separator),separator);assert!(super::blank(separator));}
        assert_eq!(super::strip("\0 \t Alice \n\0"),"Alice");
    }
}
