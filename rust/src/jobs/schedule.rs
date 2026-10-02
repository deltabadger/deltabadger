//! Rails' recurring schedules (config/recurring.yml, Fugit cron, UTC), derived the way the bots' checkpoints are
//! (engine/schedule.rs): nothing is stored but each job's last success (state.rs), and the due time follows from it.
//! Only the two cron shapes this app's jobs use.
use chrono::{DateTime, Duration, NaiveTime, Timelike, Utc};
use rand::Rng;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Schedule {
    /// `"M H * * *"`.
    Daily { hour: u32, minute: u32 },
    /// `"M */N * * *"`, N dividing 24: minute M of every hour that is a multiple of N.
    EveryHours { every: u32, minute: u32 },
}

impl Schedule {
    pub fn period(&self) -> Duration {
        match *self { Self::Daily { .. } => Duration::hours(24), Self::EveryHours { every, .. } => Duration::hours(every as i64) }
    }

    /// The latest fire at or before `now`.
    pub fn last_fire(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        let (hour, minute) = match *self {
            Self::Daily { hour, minute } => (hour, minute),
            Self::EveryHours { every, minute } => (now.hour() / every * every, minute),
        };
        let t = now.date_naive().and_time(NaiveTime::from_hms_opt(hour, minute, 0).expect("a valid schedule")).and_utc();
        if t > now { t - self.period() } else { t }
    }

    /// The first fire after `now`.
    pub fn next_fire(&self, now: DateTime<Utc>) -> DateTime<Utc> { self.last_fire(now) + self.period() }

    /// About two missed fires: two periods plus an hour for a late run. A source this job refreshes is stale past it
    /// (engine::staleness), and the runner lets no woken job run ahead of this job once its fire is that overdue.
    pub fn stale_after(&self) -> Duration { self.period() * 2 + Duration::hours(1) }
}

/// Seconds added to each fire, drawn afresh for every fire: `rand(min..=max)`. The fleet's jobs share one data-api, so
/// Rails spreads its bulk pulls (sync_stocks_from_deltabadger_job.rb:20-36, fetch_all_assets_data_from_coingecko_job.rb:14-17,
/// sync_all_tickers_and_assets_job.rb:5-9).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Jitter { pub min_secs: i64, pub max_secs: i64 }

impl Jitter {
    pub const NONE: Self = Self { min_secs: 0, max_secs: 0 };

    pub fn draw(&self) -> Duration {
        Duration::seconds(if self.max_secs > self.min_secs { rand::thread_rng().gen_range(self.min_secs..=self.max_secs) } else { self.min_secs })
    }
}

/// When a scheduled job is first due in this process. If it has had no success since its latest fire, that fire was missed
/// (Rails ran it, or nobody did, while this process did not own the install), and the job is due at that fire plus the
/// draw. A draw already past means at once: Rails anchors the jitter to the cron tick, so a late start spends the window
/// instead of extending it (sync_stocks_from_deltabadger_job.rb:89-102). Otherwise: the next fire plus the draw.
pub fn first_due(s: Schedule, draw: Duration, last_success: Option<DateTime<Utc>>, now: DateTime<Utc>) -> DateTime<Utc> {
    let last = s.last_fire(now);
    if last_success.is_none_or(|t| t < last) { last + draw } else { s.next_fire(now) + draw }
}
