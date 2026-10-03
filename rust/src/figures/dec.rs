//! The decimal the figures hold: Ruby's BigDecimal (3.3.1) as Rails' accounting uses it. Exact sums, differences
//! and products; a quotient rounded to a precision that depends on its operands; a negative zero, which Rails
//! prints (`"-0.0"`).
//!
//! It is held as Ruby holds it: limbs of nine decimal digits, aligned on the decimal point. So a sum costs what
//! its operands are long, a product of a long number and a short one costs the long one's length, and writing a
//! number out costs its length, as in Ruby. That matters because Rails' accounting grows its numbers: every sale
//! multiplies a holding's cost by a quotient of some thirty digits, and after a thousand sales the cost is a
//! number of thirty thousand digits that every later order adds to.
//!
//! Three things keep a hostile or a runaway number from costing more than it should:
//! - what comes in is measured (`entering`): the limits below, the engine's own;
//! - every operation is charged to the figure's budget before it runs, and every number alive is counted against
//!   what the figure may hold (`budget`), both in limbs;
//! - the zeros between a computed number and the point are charged too, before they can be printed.
//!
//! A `Dec` can only be made by the constructors of this file. It is shared, not copied: a clone is a reference.
use super::budget;
use super::num::{shortest, NumError};
use bigdecimal::num_bigint::BigUint;
use rusqlite::types::ValueRef;
use serde_json::Value;
use std::cmp::Ordering;
use std::sync::Arc;

/// The input bounds belong to the engine; figures use its bounded constructors at their boundary.
pub use crate::ruby::{MAX_INPUT_LEN as MAX_TEXT, MAX_EXPONENT, MAX_DIGITS, MAX_SCALE,
                      VENUE_MAX_EXPONENT as MAX_VENUE_EXPONENT, VENUE_MAX_DIGITS as MAX_VENUE_DIGITS};

/// Where a number comes in from, which is what it is measured against.
#[derive(Clone, Copy)]
enum Door { Database, Venue }

const BASE: u64 = 1_000_000_000;
const FIG: usize = 9;

/// A magnitude: `limbs[i] x 1e9^(i + exp)`, summed. No zero limb at either end; no limbs at all is zero.
#[derive(Debug)]
struct Mag { limbs: Vec<u32>, exp: i64 }

impl Mag {
    fn zero() -> Mag { Mag { limbs: vec![], exp: 0 } }
    fn trimmed(mut limbs: Vec<u32>, mut exp: i64) -> Mag {
        while limbs.last() == Some(&0) { limbs.pop(); }
        let low = limbs.iter().take_while(|limb| **limb == 0).count();
        if low > 0 { limbs.drain(..low); exp += low as i64; }
        if limbs.is_empty() { Mag::zero() } else { Mag { limbs, exp } }
    }
    fn is_zero(&self) -> bool { self.limbs.is_empty() }
    fn len(&self) -> u64 { self.limbs.len() as u64 }
    /// The place above the highest limb.
    fn top(&self) -> i64 { self.exp + self.limbs.len() as i64 }
    /// The limb at place `at`, zero outside the number.
    fn limb(&self, at: i64) -> u32 {
        usize::try_from(at - self.exp).ok().and_then(|i| self.limbs.get(i)).copied().unwrap_or(0)
    }

    fn compare(&self, o: &Mag) -> Ordering {
        match (self.is_zero(), o.is_zero()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            (false, false) => {}
        }
        if self.top() != o.top() { return self.top().cmp(&o.top()); }
        let low = self.exp.min(o.exp);
        (low..self.top()).rev().map(|at| self.limb(at).cmp(&o.limb(at))).find(|order| order.is_ne()).unwrap_or(Ordering::Equal)
    }

    /// The limbs a sum or a difference of the two spans.
    fn span(&self, o: &Mag) -> u64 {
        if self.is_zero() { return o.len(); }
        if o.is_zero() { return self.len(); }
        (self.top().max(o.top()) - self.exp.min(o.exp)) as u64
    }

    fn plus(&self, o: &Mag) -> Mag {
        if self.is_zero() { return Mag { limbs: o.limbs.clone(), exp: o.exp }; }
        if o.is_zero() { return Mag { limbs: self.limbs.clone(), exp: self.exp }; }
        let (low, high) = (self.exp.min(o.exp), self.top().max(o.top()));
        let mut limbs = Vec::with_capacity((high - low) as usize + 1);
        let mut carry = 0u64;
        for at in low..high {
            let sum = u64::from(self.limb(at)) + u64::from(o.limb(at)) + carry;
            limbs.push((sum % BASE) as u32);
            carry = sum / BASE;
        }
        limbs.push(carry as u32);
        Mag::trimmed(limbs, low)
    }

    /// `self - o`, for `self >= o`.
    fn minus(&self, o: &Mag) -> Mag {
        if o.is_zero() { return Mag { limbs: self.limbs.clone(), exp: self.exp }; }
        let (low, high) = (self.exp.min(o.exp), self.top());
        let mut limbs = Vec::with_capacity((high - low) as usize);
        let mut borrow = 0u64;
        for at in low..high {
            let (a, b) = (u64::from(self.limb(at)), u64::from(o.limb(at)) + borrow);
            if a >= b { limbs.push((a - b) as u32); borrow = 0; } else { limbs.push((a + BASE - b) as u32); borrow = 1; }
        }
        Mag::trimmed(limbs, low)
    }

    fn times(&self, o: &Mag) -> Mag {
        if self.is_zero() || o.is_zero() { return Mag::zero(); }
        let mut limbs = vec![0u32; self.limbs.len() + o.limbs.len()];
        for (i, a) in self.limbs.iter().enumerate() {
            let mut carry = 0u64;
            for (j, b) in o.limbs.iter().enumerate() {
                let product = u64::from(*a) * u64::from(*b) + u64::from(limbs[i + j]) + carry;
                limbs[i + j] = (product % BASE) as u32;
                carry = product / BASE;
            }
            limbs[i + o.limbs.len()] = carry as u32;
        }
        Mag::trimmed(limbs, self.exp + o.exp)
    }

    /// The number as `digits x 10^-scale`: its digits with no zero at either end. Zero is no digits.
    fn digits(&self) -> (String, i64) {
        let mut digits = String::with_capacity(self.limbs.len() * FIG);
        for (i, limb) in self.limbs.iter().rev().enumerate() {
            if i == 0 { digits.push_str(&limb.to_string()); } else { digits.push_str(&format!("{limb:09}")); }
        }
        let kept = digits.trim_end_matches('0').len();
        let scale = -self.exp * FIG as i64 - (digits.len() - kept) as i64;
        digits.truncate(kept);
        (digits, scale)
    }

    /// `digits x 10^-scale`, for ASCII digits.
    fn from_digits(digits: &str, scale: i64) -> Mag {
        // Zeros behind, until the digits end on a limb's edge.
        let pad = (FIG as i64 - scale.rem_euclid(FIG as i64)) % FIG as i64;
        let padded = format!("{digits}{}", "0".repeat(pad as usize));
        let limbs = padded.as_bytes().rchunks(FIG).map(|chunk| chunk.iter().fold(0u32, |limb, digit| limb * 10 + u32::from(digit - b'0'))).collect();
        Mag::trimmed(limbs, -(scale + pad) / FIG as i64)
    }
}

/// Significant digits as BigDecimal#precision counts them: a pure fraction's leading zeros and a whole number's
/// trailing zeros count.
fn precision(digits: &str, scale: i64) -> i64 {
    let n = digits.len() as i64;
    if scale <= 0 { n - scale } else { n.max(scale) }
}

/// The digits plus one in their last place.
fn incremented(digits: &str) -> String {
    let mut bytes = digits.as_bytes().to_vec();
    for digit in bytes.iter_mut().rev() {
        if *digit == b'9' { *digit = b'0'; } else { *digit += 1; return String::from_utf8_lossy(&bytes).into_owned(); }
    }
    format!("1{}", String::from_utf8_lossy(&bytes))
}

/// A magnitude that a `Dec` holds: counted among the limbs alive (`budget`) from the moment it is shared until
/// the last `Dec` that shares it is dropped.
#[derive(Debug)]
struct Held(Mag);

impl Drop for Held { fn drop(&mut self) { budget::release(self.0.len()); } }
impl std::ops::Deref for Held { type Target = Mag; fn deref(&self) -> &Mag { &self.0 } }

fn shared(mag: Mag) -> Arc<Held> {
    budget::hold(mag.len());
    Arc::new(Held(mag))
}

#[derive(Clone, Debug)]
pub struct Dec { mag: Arc<Held>, negative: bool }

fn zero_division() -> NumError { NumError::Raised("ZeroDivisionError: divided by 0".into()) }

/// A text that passed for a clean decimal: its digits with no zero at either end, and the scale they stand at.
/// None when the exponent is too long to be one.
fn parsed(text: &str) -> Option<(bool, String, i64)> {
    let negative = text.starts_with('-');
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (mantissa, exponent) = unsigned.split_once(['e', 'E']).map_or((unsigned, "0"), |(m, e)| (m, e));
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let exponent: i64 = exponent.strip_prefix('+').unwrap_or(exponent).parse().ok()?;
    let scale = (fraction.len() as i64).checked_sub(exponent)?;
    let all = format!("{whole}{fraction}");
    let kept = all.trim_end_matches('0');
    let scale = scale.checked_sub((all.len() - kept.len()) as i64)?;
    Some((negative, kept.trim_start_matches('0').to_string(), scale))
}

fn clean(text: &str) -> bool {
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (mantissa, exponent) = unsigned.split_once(['e', 'E']).map_or((unsigned, None), |(m, e)| (m, Some(e)));
    let (whole, fraction) = mantissa.split_once('.').map_or((mantissa, None), |(w, f)| (w, Some(f)));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    digits(whole) && fraction.is_none_or(digits) && exponent.is_none_or(|e| digits(e.strip_prefix(['-', '+']).unwrap_or(e)))
}

impl Dec {
    /// A computed number. A zero keeps the sign it was computed with.
    fn computed(mag: Mag, negative: bool) -> Result<Dec, NumError> {
        // The gap between the number and the decimal point: zeros that are no limb of it and would be written out.
        let gap = if mag.is_zero() { 0 } else { mag.exp.max(-mag.top()).max(0) as u64 };
        budget::charge(gap.saturating_mul(FIG as u64), 0)?;
        Ok(Dec { mag: shared(mag), negative })
    }

    pub fn zero() -> Dec { Dec { mag: shared(Mag::zero()), negative: false } }
    pub fn one() -> Dec { Dec::from_i64(1) }
    pub fn from_i64(i: i64) -> Dec {
        Dec { mag: shared(Mag::from_digits(&i.unsigned_abs().to_string(), 0)), negative: i < 0 }
    }

    /// Float#to_d: the shortest digits that read back as the Float, cut (not rounded) at sixteen.
    fn float_digits(f: f64) -> Result<(String, i64), NumError> {
        if !f.is_finite() { return Err(NumError::NotANumber); }
        let (_, digits, exponent) = shortest(f);
        let cut = &digits[..digits.len().min(16)];
        let kept = cut.trim_end_matches('0');
        Ok((kept.trim_start_matches('0').to_string(), kept.len() as i64 - 1 - i64::from(exponent)))
    }

    /// Float#to_d. A Float is at most 16 digits and 308 places from the point.
    pub fn from_f64(f: f64) -> Result<Dec, NumError> {
        Dec::real(f, Door::Database)
    }

    /// A decimal as a venue, a provider or a column writes one: a sign, digits, a fraction, an exponent, and
    /// nothing else. Ruby's String#to_d reads the same number from such a text; from any other it reads a number
    /// off the front ("12abc" is 12, "abc" is 0) or a BigDecimal that is no number ("NaN", "Infinity"), and a figure
    /// made from either would be a wrong figure, so here it is `NumError::NotANumber`.
    pub fn strict(text: &str) -> Result<Dec, NumError> { Dec::read(text, Door::Database) }

    fn read(text: &str, door: Door) -> Result<Dec, NumError> {
        if text.len() > MAX_TEXT { return Err(NumError::OutOfRange); }
        if !clean(text) { return Err(NumError::NotANumber); }
        // The engine owns both sets of caps. The grammar above and the sign of zero belong to figures.
        match door {
            Door::Database => { crate::ruby::BigDec::parse(text).map_err(|_| NumError::OutOfRange)?; }
            Door::Venue => { crate::ruby::json_to_d(&Value::String(text.to_string())).map_err(|_| NumError::OutOfRange)?; }
        }
        let (negative, digits, scale) = parsed(text).ok_or(NumError::OutOfRange)?;
        Ok(Dec { mag: shared(Mag::from_digits(&digits, scale)), negative })
    }

    /// `value.to_d` on a value of a venue's or a provider's JSON: nil is zero (as in Ruby), an Integer is exact, a
    /// Float is Float#to_d, a String is read strictly. Anything else has no `to_d` in Ruby. Within the venue's limits.
    pub fn to_d(v: &Value) -> Result<Dec, NumError> {
        match v {
            Value::Null => Ok(Dec::zero()),
            Value::Number(n) if n.is_f64() => Dec::real(n.as_f64().ok_or(NumError::NotANumber)?, Door::Venue),
            Value::Number(n) => Dec::read(&n.to_string(), Door::Venue),
            Value::String(s) => Dec::read(s, Door::Venue),
            _ => Err(NumError::NotANumber),
        }
    }

    /// A Float on its way in (a REAL column, a JSON number with a fraction): Float#to_d.
    fn real(f: f64, door: Door) -> Result<Dec, NumError> {
        let (digits, scale) = Dec::float_digits(f)?;
        match door {
            Door::Database => { crate::ruby::BigDec::from_f64(f).map_err(|_| NumError::OutOfRange)?; }
            Door::Venue => { crate::ruby::json_to_d(&serde_json::json!(f)).map_err(|_| NumError::OutOfRange)?; }
        }
        Ok(Dec { mag: shared(Mag::from_digits(&digits, scale)), negative: f.is_sign_negative() })
    }

    /// A decimal column from SQLite (NUMERIC affinity: INTEGER, REAL or TEXT). SQLite itself stores a numeric text
    /// as a number, so a text here is one it could not read; ActiveRecord would read it as String#to_d does.
    pub fn from_sql(v: ValueRef<'_>) -> Result<Option<Dec>, NumError> {
        match v {
            ValueRef::Null => Ok(None),
            ValueRef::Integer(i) => Ok(Some(Dec::from_i64(i))),
            ValueRef::Real(f) => Dec::real(f, Door::Database).map(Some),
            ValueRef::Text(t) if t.len() > MAX_TEXT => Err(NumError::OutOfRange),
            ValueRef::Text(t) => Dec::strict(std::str::from_utf8(t).map_err(|_| NumError::NotANumber)?).map(Some),
            ValueRef::Blob(_) => Err(NumError::NotANumber),
        }
    }

    /// A number written out by this library or by Ruby, read back (the vectors): of any length the budget affords.
    pub fn parse(text: &str) -> Result<Dec, NumError> {
        let text = text.trim();
        let limbs = (text.len() / FIG) as u64 + 1;
        budget::charge(limbs, limbs)?;
        if !clean(text) { return Err(NumError::NotANumber); }
        let (negative, digits, scale) = parsed(text).ok_or(NumError::OutOfRange)?;
        Dec::computed(Mag::from_digits(&digits, scale), negative)
    }

    pub fn is_zero(&self) -> bool { self.mag.is_zero() }
    pub fn is_positive(&self) -> bool { !self.negative && !self.mag.is_zero() }
    pub fn is_negative(&self) -> bool { self.negative && !self.mag.is_zero() }
    fn opposite(&self) -> Dec { Dec { mag: self.mag.clone(), negative: !self.negative } }

    /// BigDecimal#/ (BigDecimal_div2 with n = 0): the quotient to as many digits as the longer operand has, plus
    /// sixteen (at least 32), the last of them rounded half up. Ruby gives Infinity for a zero divisor; Rails'
    /// accounting never divides by one without raising first, and neither does this.
    pub fn div(&self, o: &Dec) -> Result<Dec, NumError> {
        if o.is_zero() { return Err(zero_division()); }
        let negative = self.negative != o.negative;
        if self.is_zero() { return Ok(Dec { mag: shared(Mag::zero()), negative }); }
        budget::charge(self.mag.len() + o.mag.len(), 0)?;
        let ((a, a_scale), (b, b_scale)) = (self.mag.digits(), o.mag.digits());
        let kept = (precision(&a, a_scale).max(precision(&b, b_scale)) + 16).max(32);
        // Enough zeros behind the dividend for `kept` + 1 digits of quotient.
        let shift = (kept + 2 + b.len() as i64 - a.len() as i64).max(0);
        // A long division, through binary and back. In limbs: the quotient's times the divisor's for the division,
        // and an eighth of each number's square for reading it into binary or writing it out of it.
        let limbs = |digits: i64| (digits / FIG as i64) as u64 + 1;
        let (dividend, divisor, quotient) = (limbs(a.len() as i64 + shift), limbs(b.len() as i64), limbs(kept) + 1);
        let square = |n: u64| n.saturating_mul(n) / 8;
        budget::charge(quotient.saturating_mul(divisor).saturating_add(square(dividend)).saturating_add(square(divisor)).saturating_add(square(quotient)), quotient)?;
        let dividend = format!("{a}{}", "0".repeat(shift as usize));
        let (Some(dividend), Some(divisor)) = (BigUint::parse_bytes(dividend.as_bytes(), 10), BigUint::parse_bytes(b.as_bytes(), 10)) else { return Err(NumError::NotANumber) };
        let quotient = (dividend / divisor).to_string();
        let mut scale = a_scale - b_scale + shift;
        let digits = if quotient.len() as i64 > kept {
            let (head, rest) = quotient.split_at(kept as usize);
            scale -= rest.len() as i64;
            if rest.as_bytes()[0] >= b'5' { incremented(head) } else { head.to_string() }
        } else { quotient };
        let whole = digits.trim_start_matches('0');
        Dec::computed(Mag::from_digits(whole, scale), negative)
    }

    /// BigDecimal#round(places), half away from zero; a zero that was rounded from below keeps its sign.
    pub fn round(&self, places: i64) -> Result<Dec, NumError> {
        crate::ruby::scale(places).map_err(|_| NumError::OutOfRange)?;
        if -self.mag.exp * FIG as i64 <= places { return Ok(self.clone()); }
        budget::charge(self.mag.len(), self.mag.len())?;
        let (digits, scale) = self.mag.digits();
        if scale <= places { return Ok(self.clone()); }
        let cut = digits.len() as i64 - (scale - places);
        let head = if cut > 0 { &digits[..cut as usize] } else { "0" };
        let up = cut >= 0 && digits.as_bytes()[cut as usize] >= b'5';
        let rounded = if up { incremented(head) } else { head.to_string() };
        Dec::computed(Mag::from_digits(rounded.trim_start_matches('0'), places), self.negative)
    }

    /// BigDecimal#to_s in plain notation, as Rails prints one: at least one fractional digit.
    pub fn to_s_f(&self) -> String {
        budget::step(self.mag.len().saturating_add(self.mag.exp.max(-self.mag.top()).max(0) as u64));
        let sign = if self.negative { "-" } else { "" };
        let (digits, scale) = self.mag.digits();
        let n = digits.len() as i64;
        if n == 0 { return format!("{sign}0.0"); }
        if scale <= 0 { return format!("{sign}{digits}{}.0", "0".repeat(-scale as usize)); }
        if n > scale { return format!("{sign}{}.{}", &digits[..(n - scale) as usize], &digits[(n - scale) as usize..]); }
        format!("{sign}0.{}{digits}", "0".repeat((scale - n) as usize))
    }
    /// BigDecimal#to_f: the correctly rounded double.
    pub fn to_f(&self) -> f64 { self.to_s_f().parse().unwrap_or(f64::NAN) }
}

// Sums, differences and products are exact. Each is charged for the limbs it will write before it writes them.
impl std::ops::Add for &Dec {
    type Output = Result<Dec, NumError>;
    fn add(self, o: &Dec) -> Result<Dec, NumError> {
        let span = self.mag.span(&o.mag) + 1;
        budget::charge(span, span)?;
        // -0 + -0 is -0; any other sum that is zero is +0.
        if self.negative == o.negative { return Dec::computed(self.mag.plus(&o.mag), self.negative); }
        match self.mag.compare(&o.mag) {
            Ordering::Equal => Ok(Dec::zero()),
            Ordering::Greater => Dec::computed(self.mag.minus(&o.mag), self.negative),
            Ordering::Less => Dec::computed(o.mag.minus(&self.mag), o.negative),
        }
    }
}
impl std::ops::Sub for &Dec {
    type Output = Result<Dec, NumError>;
    #[allow(clippy::suspicious_arithmetic_impl)] // a difference is the sum with the sign turned, -0 and all
    fn sub(self, o: &Dec) -> Result<Dec, NumError> { self + &o.opposite() }
}
impl std::ops::Mul for &Dec {
    type Output = Result<Dec, NumError>;
    fn mul(self, o: &Dec) -> Result<Dec, NumError> {
        let (a, b) = (self.mag.len(), o.mag.len());
        budget::charge(a.saturating_mul(b).saturating_add(a + b), a + b)?;
        Dec::computed(self.mag.times(&o.mag), self.negative != o.negative)
    }
}

// Equal and ordered by value: the two zeros are one number.
impl PartialEq for Dec { fn eq(&self, o: &Dec) -> bool { self.cmp(o) == Ordering::Equal } }
impl Eq for Dec {}
impl PartialOrd for Dec { fn partial_cmp(&self, o: &Dec) -> Option<Ordering> { Some(self.cmp(o)) } }
impl Ord for Dec {
    fn cmp(&self, o: &Dec) -> Ordering {
        budget::step(1);
        match (self.is_negative(), o.is_negative()) {
            (false, false) => self.mag.compare(&o.mag),
            (true, true) => o.mag.compare(&self.mag),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        }
    }
}
