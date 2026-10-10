//! The status bar and the status button of a bot: bots/status/_status_bar.html.erb and
//! _status_button.html.erb, on a tile and on the bot page alike.
use super::{start, Bot, Kind};
use crate::engine::schedule::Unrounded;
use crate::engine::venue_rules;
use crate::enums::BotStatus;
use crate::ruby::to_sentence;
use crate::web::format::{float_to_s, iso8601};
use crate::web::layout::Ctx;
use crate::web::{i18n, WebError};
use askama::Template;
use bigdecimal::num_bigint::{BigInt, Sign};
use bigdecimal::ToPrimitive;
use chrono::{DateTime, Utc};
use rusqlite::Connection;

/// Exchange#humanize_error for a venue without a Honeymaker classifier (Alpaca): the sentence of
/// the bucket the message falls in (Exchange::KIND_ERROR_KEYS), else the message as it came.
fn humanize(bot: &Bot, message: &str, locale: &str) -> String {
    let kind = venue_rules::for_exchange(&bot.exchange.class).and_then(|rules| rules.failure_kind(&[message.to_string()]));
    let key = match kind {
        Some("throttle") => "rate_limited",
        Some("transient") => "transient_unavailable",
        Some(kind @ ("insufficient_funds" | "invalid_key" | "permission_denied" | "restricted")) => kind,
        _ => return message.to_string(),
    };
    i18n::text(locale, &format!("errors.exchange.{key}"), &[("exchange", i18n::Arg::Text(&bot.exchange.name))])
}

/// BotHelper#humanized_bot_error: each message in the reader's words, joined as an English sentence.
pub fn humanized_error(bot: &Bot, messages: &[String], locale: &str) -> Option<String> {
    let list: Vec<String> = messages.iter().filter(|message| !message.trim().is_empty()).map(|message| humanize(bot, message, locale)).collect();
    (!list.is_empty()).then(|| to_sentence(&list))
}

/// An instant as Ruby's Time holds it: exact. A time out of a row is on the microsecond grid, but
/// a checkpoint is such a time plus Floats (`Time + Float` adds the Float's exact binary value), and
/// Rails prints, compares and stores it from that exact value. Nanoseconds, as a whole number over
/// a power of two.
#[derive(Clone, Debug)]
pub struct Instant {
    num: BigInt,
    bits: u32,
}

/// A Float as its whole mantissa and its power of two; `None` for one that is not a number.
fn mantissa(f: f64) -> Option<(i64, i32)> {
    if !f.is_finite() { return None; }
    let bits = f.to_bits();
    let (exponent, fraction) = (((bits >> 52) & 0x7ff) as i32, (bits & ((1 << 52) - 1)) as i64);
    let (whole, power) = if exponent == 0 { (fraction, -1074) } else { (fraction | (1 << 52), exponent - 1075) };
    Some((if f < 0.0 { -whole } else { whole }, power))
}

/// MRI's Integer#fdiv for two positive Integers (`rb_int_fdiv_double`, bignum.c), which is how a
/// Rational becomes a Float there. It is not the nearest Float to the quotient: an Integer of 62
/// bits or fewer is converted on its own and the two Floats divided, and past that the divisor
/// keeps its top 64 bits and the quotient is cut off before it is converted. Ported step by step
/// and held to Ruby by vectors; the nearest Float differs in more than a quarter of them.
fn fdiv(x: &BigInt, y: &BigInt) -> f64 {
    let float = |v: &BigInt| v.to_f64().unwrap_or(f64::INFINITY);
    if y.bits() <= 62 { return float(x) / float(y); }
    let shift = |v: &BigInt, by: i64| if by > 0 { v >> by as u64 } else { v << (-by) as u64 };
    let ey = y.bits() as i64 - 64;
    let mut ex = x.bits() as i64 - 128;
    if ex > 64 { ex -= 64 } else if ex > 0 { ex = 0 }
    float(&(shift(x, ex) / shift(y, ey))) * 2f64.powi((ex - ey) as i32)
}

/// Time#to_f and Time#- of a number of nanoseconds (`rb_time_unmagnify_to_float`, time.c): whole
/// seconds exactly, whole nanoseconds as their Float over a billion, and anything finer as the
/// Rational it is, through `fdiv`.
fn nanoseconds_to_f(num: &BigInt, bits: u32) -> f64 {
    if num.sign() == Sign::NoSign { return 0.0; }
    let (negative, mut x): (bool, BigInt) = (num.sign() == Sign::Minus, num.magnitude().clone().into());
    let spare = trailing_zeros(&x).min(bits);
    x >>= spare;
    let bits = bits - spare;
    let billion = BigInt::from(1_000_000_000);
    let seconds = if bits == 0 {
        if x.bits() <= 62 && (&x % &billion).sign() == Sign::NoSign { (x / billion).to_f64().unwrap_or(f64::INFINITY) } else { x.to_f64().unwrap_or(f64::INFINITY) / 1e9 }
    } else {
        // Over 2^bits × 10^9, in lowest terms: the numerator is odd by now, so only fives are shared.
        let mut y = billion << bits;
        let five = BigInt::from(5);
        for _ in 0..9 {
            if (&x % &five).sign() != Sign::NoSign { break; }
            x /= &five;
            y /= &five;
        }
        fdiv(&x, &y)
    };
    if negative { -seconds } else { seconds }
}

fn trailing_zeros(x: &BigInt) -> u32 {
    x.trailing_zeros().map_or(0, |zeros| zeros.min(u64::from(u32::MAX)) as u32)
}

impl Instant {
    pub fn from_micros(micros: i64) -> Instant {
        Instant { num: BigInt::from(micros) * 1000, bits: 0 }
    }

    /// A checkpoint of the engine's schedule before it is rounded.
    pub fn of(checkpoint: &Unrounded) -> Instant {
        checkpoint.terms.iter().fold(Instant::from_micros(checkpoint.base_us), |instant, (float, times)| instant.plus(*float, *times))
    }

    /// `self + float`, `times` times over: exact.
    fn plus(mut self, float: f64, times: i64) -> Instant {
        // A span `Bot::unrendered` let through is a number; what is not adds nothing.
        let Some((whole, power)) = mantissa(float) else { return self };
        let added = BigInt::from(whole) * times * 1_000_000_000_i64;
        if power >= 0 {
            self.num += added << (power as u32 + self.bits);
        } else {
            let needed = power.unsigned_abs();
            if needed > self.bits {
                self.num <<= needed - self.bits;
                self.bits = needed;
            }
            self.num += added << (self.bits - needed);
        }
        self
    }

    /// The microsecond this instant is in: what a `datetime(6)` column keeps, and where `Time#iso8601` cuts.
    pub fn floor_micros(&self) -> i64 {
        let unit = BigInt::from(1000) << self.bits;
        let mut micros = &self.num / &unit;
        if self.num.sign() == Sign::Minus && (&micros * &unit) != self.num { micros -= 1; }
        micros.to_i64().unwrap_or(if self.num.sign() == Sign::Minus { i64::MIN } else { i64::MAX })
    }

    /// Time#to_f.
    pub fn to_f(&self) -> f64 {
        nanoseconds_to_f(&self.num, self.bits)
    }

    /// `self - earlier`, as Time#- gives it: a Float of seconds.
    pub fn since(&self, earlier: &Instant) -> f64 {
        let bits = self.bits.max(earlier.bits);
        nanoseconds_to_f(&((&self.num << (bits - self.bits)) - (&earlier.num << (bits - earlier.bits))), bits)
    }

    /// The time Rails holds the job of this checkpoint at, in microseconds. ActiveJob hands its
    /// adapter `wait_until.to_f`, Solid Queue reads the Float back with `Time.at`, and the column
    /// keeps six decimals, cut off. So the Float is taken of the checkpoint as it is, with whatever
    /// lies below the microsecond, and a checkpoint whose seconds are not a Float's comes out up to
    /// a microsecond early. The countdown prints whole seconds and is the same either way; the
    /// progress bar's width is a quotient of such times printed to sixteen digits, and is Rails'
    /// only with this.
    pub fn held_micros(&self) -> i64 {
        let seconds = self.to_f();
        let whole = seconds.floor();
        (whole as i64).saturating_mul(1_000_000).saturating_add(((seconds - whole) * 1_000_000.0).floor() as i64)
    }
}

/// Automation::Schedulable#progress_percentage times 100, as the bar's width prints it: a Float,
/// or the Integer 0 when there is no span to measure.
pub fn progress_width(now: DateTime<Utc>, start: Option<&Instant>, end: Option<DateTime<Utc>>) -> String {
    let (now, end) = (Instant::from_micros(now.timestamp_micros()), end.map(|end| Instant::from_micros(end.timestamp_micros())));
    match (start, end) {
        (Some(start), Some(end)) if end.since(start) > 0.0 => float_to_s(now.since(start) / end.since(start) * 100.0),
        _ => "0".to_string(),
    }
}

struct Countdown {
    /// The progress bar's width; only a scheduled bot has the bar.
    width: Option<String>,
    start: Option<String>,
    end: Option<String>,
    prefix: Option<String>,
}

#[derive(Template)]
#[template(path = "bots/status/_status_bar.html")]
struct Bar<'a> {
    v: &'a Ctx,
    id: String,
    active: bool,
    text: Option<String>,
    dots: bool,
    countdown: Option<Countdown>,
}

enum Button {
    Reactivate,
    Stop,
    /// A stopped bot that has run before: Start opens the "start now or wait" modal. Disabled when it may not start.
    Restart(bool),
    Start(bool),
}

#[derive(Template)]
#[template(path = "bots/status/_status_button.html")]
struct ButtonView<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    id: String,
    /// `bot_path(bot)`, which every action of the button extends.
    path: String,
    button: Button,
}

/// `Instant::held_micros` of a time on the microsecond grid.
pub fn job_time_us(checkpoint_us: i64) -> i64 {
    Instant::from_micros(checkpoint_us).held_micros()
}

/// When the bot acts next: Rails' `next_action_job_at`, the time its scheduled job waits for. After
/// every tick that is the next interval checkpoint, and the engine's schedule gives the same
/// instant. Two times are in no row and so are not known here: a closed market's opening, and a
/// retry the engine has in hand (listed divergences in rust/tests/pages.rs).
///
/// Rails has no time either while a tick is due or in hand: its job is then ready, claimed or
/// blocked, no longer scheduled (Automation::Schedulable#next_action_job_at). There is no job table
/// here, so "due" is read from the row: the checkpoint has come and the bot has not acted on it.
pub fn next_action_at(bot: &Bot, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>,WebError> {
    if bot.transient.get("waiting_for_market_open").is_some_and(|flag| !flag.is_null() && flag != &serde_json::Value::Bool(false)) { return Ok(None); }
    let Some(checkpoints) = bot.checkpoints(now)? else {return Ok(None)};
    let due = match bot.last_action_job_at()? {
        // Bot::ActionJob writes `last_action_job_at` as it starts a tick: an older one means the last checkpoint's tick has not
        // started. The stored time is cut to the millisecond and a checkpoint is not, so the two are compared in milliseconds,
        // as the engine compares them (engine::run): a tick that began within its checkpoint's millisecond has begun.
        Some(acted) => acted.timestamp_millis() < checkpoints.last_us.div_euclid(1000),
        // A start clears it: until the first tick runs, a bot whose start has come is due.
        None => bot.anchor()?.is_some_and(|anchor| anchor <= now),
    };
    // On the grid to the microsecond, the checkpoint is this instant.
    if due || checkpoints.next_us <= now.timestamp_micros() { return Ok(None); }
    // The job is enqueued for the checkpoint as it is before any rounding.
    let Some((next, _)) = bot.unrounded(now)? else {return Ok(None)};
    Ok(DateTime::from_timestamp_micros(Instant::of(&next).held_micros()))
}

pub struct Status {
    pub bar: String,
    pub button: String,
    /// What the start validation found, when the button asked for it (a bot that is created or
    /// stopped): the forms below print its messages under their fields.
    pub check: Option<start::Check>,
}

pub fn render(c: &Connection, ctx: &Ctx, csrf: &str, bot: &Bot, market_data_configured: bool) -> Result<Status, WebError> {
    let t = |key: &str| i18n::text(ctx.locale, key, &[]);
    let (mut text, mut dots, mut countdown) = (None, false, None);
    match bot.status {
        BotStatus::Archived => text = Some(t("bot.status.archived")),
        BotStatus::Executing => text = Some(t("bot.status.setting_orders")),
        _ if bot.excess_members() > 0 => {
            text = Some(i18n::text(ctx.locale, "bot.dca_multi_asset.too_many_assets", &[("count", i18n::Arg::Count(bot.excess_members() as i64))]));
        }
        _ if bot.kind == Kind::Basket && !bot.allocations_balanced() => text = Some(t("bot.dca_multi_asset.normalize_first")),
        BotStatus::Stopped => {
            text = Some(match bot.stop_message_key.as_deref().filter(|key| !key.trim().is_empty()) {
                Some(key) => format!("{}: {}", t("bot.status.paused"), t(key)),
                None => t("bot.status.paused"),
            });
        }
        BotStatus::Waiting => (text, dots) = (Some(t("bot.status.waiting")), true),
        BotStatus::Scheduled | BotStatus::Retrying => {
            let scheduled = bot.status == BotStatus::Scheduled;
            let end = next_action_at(bot, ctx.now)?;
            // `last_action_job_at || last_interval_checkpoint_at`: the second is not on the microsecond grid, and Rails measures from where it is.
            let start = bot.last_action_job_at()?.map(|at| Instant::from_micros(at.timestamp_micros())).or(bot.unrounded(ctx.now)?.map(|(_, last)| Instant::of(&last)));
            let failure = bot.last_order.as_ref().filter(|order| order.failed).and_then(|order| humanized_error(bot, &order.error_messages, ctx.locale));
            let prefix = if !scheduled {
                Some(match failure {
                    Some(error) => format!("{} - {}", i18n::text(ctx.locale, "bot.messages.failed_status_bar_explanation", &[("error_message", i18n::Arg::Text(&error))]), t("bot.status.next_try")),
                    None => t("bot.status.next_try"),
                })
            } else {
                bot.transient.get("waiting_for_market_open").filter(|flag| !flag.is_null() && **flag != serde_json::Value::Bool(false)).map(|_| t("bot.status.market_closed"))
            };
            countdown = Some(Countdown {
                width: scheduled.then(|| progress_width(ctx.now, start.as_ref(), end)),
                start: start.as_ref().and_then(|start| DateTime::from_timestamp_micros(start.floor_micros())).map(iso8601), end: end.map(iso8601), prefix,
            });
        }
        BotStatus::Created | BotStatus::Deleted => {}
    }
    let bar = Bar { v: ctx, id: bot.dom_id("status_bar"), active: bot.status == BotStatus::Scheduled, text, dots, countdown }.render()?;
    let mut check = None;
    let button = match bot.status {
        BotStatus::Archived => Button::Reactivate,
        BotStatus::Stopped | BotStatus::Created => {
            let found = start::check(c, bot, ctx.now, market_data_configured, ctx.locale)?;
            let disabled = found.invalid || !bot.api_key_correct();
            check = Some(found);
            if bot.restarting() { Button::Restart(disabled) } else { Button::Start(disabled) }
        }
        _ => Button::Stop,
    };
    let button = ButtonView { v: ctx, csrf, id: bot.dom_id("status_button"), path: ctx.path(&format!("/bots/{}", bot.id)), button }.render()?;
    Ok(Status { bar, button, check })
}

/// ActionCable rendering has no browser session. Turbo uses the receiving page's CSRF header.
pub(super) fn broadcast(c: &Connection, ctx: &Ctx, bot: &Bot, configured: bool) -> Result<Status, WebError> {
    let mut state = render(c, ctx, "", bot, configured)?;
    state.button = state.button.replace("<input type=\"hidden\" name=\"authenticity_token\" value=\"\" />", "");
    Ok(state)
}
