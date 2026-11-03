//! AppUpdate and AppUpdate::CheckJob (app/models/app_update.rb, app/jobs/app_update/check_job.rb, clients/github.rb,
//! installation.rb): twice a day, ask GitHub for the latest release and keep the answer for the Settings row
//! (web::settings::sections, settings/widgets/_app_update). Desktop installs update themselves and opted-out installs
//! (DELTABADGER_UPDATE_CHECK=false, which launchpad sets on every hosted container) never ask.
//!
//! Rails keeps the answer in Rails.cache, a database in production (solid_cache), for 25 hours. Here it is one plain
//! `app_configs` row (KEY) with the time it was fetched; a read past TTL is no answer. A failed run never touches it.
use super::schedule::{Jitter, Schedule};
use super::{Cx, Job, JobFuture, Outcome, Retry, Spec, Wake, DEADLINE};
use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::future::Future;
use std::pin::Pin;

pub const NAME: &str = "app_update_check_job";
/// The row: `{"version","url","checked_at"}`.
pub const KEY: &str = "rust_app_update";
/// AppUpdate::CACHE_TTL: an hour longer than the schedule, so an answer survives one missed run.
pub fn ttl() -> Duration { Duration::hours(25) }
/// AppUpdate::COMPOSE_COMMAND.
pub const COMPOSE_COMMAND: &str = "docker compose pull && docker compose up -d";
pub const LATEST: &str = "https://api.github.com/repos/deltabadger/deltabadger/releases/latest";
pub const RELEASES_URL: &str = "https://github.com/deltabadger/deltabadger/releases";
pub const MANUAL_URL: &str = "https://github.com/deltabadger/deltabadger/blob/main/manual";
/// A release answer is a few KB; anything this size is not one.
const MAX_BODY: usize = 1 << 20;

/// Installation.platform: DELTABADGER_PLATFORM, declared by each launcher and never inferred. Anything else is Unknown
/// (Rails logs a warning), which the Settings row answers with the manual.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform { Desktop, Umbrel, Docker, Unknown }

pub fn platform(env: &dyn Fn(&str) -> Option<String>) -> Platform {
    let marker = env("DELTABADGER_PLATFORM").unwrap_or_default().trim().to_lowercase();
    match marker.as_str() {
        "" => Platform::Unknown,
        "desktop" => Platform::Desktop,
        "umbrel" => Platform::Umbrel,
        "docker" => Platform::Docker,
        other => {
            crate::engine::log(&format!("Unrecognised DELTABADGER_PLATFORM={other:?}; expected one of desktop, umbrel, docker. Treating this install as unknown."));
            Platform::Unknown
        }
    }
}

/// AppUpdate.enabled?: never on the desktop, and not where DELTABADGER_UPDATE_CHECK reads as false (env_boolean).
pub fn enabled(env: &dyn Fn(&str) -> Option<String>) -> bool {
    platform(env) != Platform::Desktop && crate::web::env_boolean(env("DELTABADGER_UPDATE_CHECK")) != Some(false)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Release { pub version: String, pub url: String }

#[derive(Serialize, Deserialize)]
struct Stored { version: String, url: String, checked_at: String }

/// Clients::Github#latest_release on an answer: Faraday parses a JSON body whatever the status, `raise_error` refuses
/// 4xx/5xx, and the tag must be a plain three-part version. The link is built from the tag, never taken from the payload.
pub fn release_from(status: u16, content_type: &str, body: &[u8]) -> Option<Release> {
    if status >= 400 { return None; }
    // Faraday's :json response middleware: the media type (before any `;`) ending in json.
    let mime = content_type.split(';').next().unwrap_or_default().trim();
    if !regex::Regex::new(r"\bjson$").expect("valid").is_match(mime) { return None; }
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let tag = match &v.as_object()?.get("tag_name") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    let version = regex::Regex::new(r"^v?([0-9]+\.[0-9]+\.[0-9]+)$").expect("valid").captures(&tag)?.get(1)?.as_str().to_string();
    Some(Release { url: format!("{RELEASES_URL}/tag/{tag}"), version })
}

/// AppUpdate.latest: the stored answer while it is younger than TTL at `now`.
pub fn latest(c: &Connection, now: DateTime<Utc>) -> Result<Option<Release>, String> {
    let Some(text) = crate::app_config::get_plain(c, KEY)? else { return Ok(None) };
    let Ok(s) = serde_json::from_str::<Stored>(&text) else { return Ok(None) };
    let Ok(at) = crate::codec::parse_time(&s.checked_at) else { return Ok(None) };
    Ok((now - at < ttl()).then_some(Release { version: s.version, url: s.url }))
}

pub fn store(c: &Connection, release: &Release, now: DateTime<Utc>) -> Result<(), String> {
    let s = Stored { version: release.version.clone(), url: release.url.clone(), checked_at: crate::codec::format_time(now) };
    crate::app_config::set_plain(c, KEY, &serde_json::to_string(&s).expect("serializable"), now)
}

/// AppUpdate.available?: `Gem::Version.new(latest) > Gem::Version.new(current)`, false when either is malformed.
pub fn available(current: &str, latest: &str) -> bool {
    match (gem::Version::parse(latest), gem::Version::parse(current)) { (Some(l), Some(c)) => l.cmp(&c) == Ordering::Greater, _ => false }
}

/// Gem::Version (rubygems/version.rb), as far as comparing needs.
mod gem {
    use std::cmp::Ordering;

    #[derive(Clone, Debug, PartialEq)]
    enum Seg { Num(String), Str(String) }

    pub struct Version(Vec<Seg>);

    impl Version {
        /// `correct?` (ANCHORED_VERSION_PATTERN), then: blank → "0", strip, `-` → `.pre.`, segments by /[0-9]+|[a-z]+/i,
        /// trailing zeros dropped from the numeric part and the string part (canonical_segments).
        pub fn parse(s: &str) -> Option<Self> {
            let anchored = regex::Regex::new(r"^\s*([0-9]+(?:\.[0-9a-zA-Z]+)*(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?)?\s*$").expect("valid");
            if !anchored.is_match(s) { return None; }
            let v = if s.trim().is_empty() { "0".to_string() } else { s.trim().replace('-', ".pre.") };
            let segs: Vec<Seg> = regex::Regex::new(r"[0-9]+|[a-zA-Z]+").expect("valid").find_iter(&v)
                .map(|m| if m.as_str().bytes().all(|b| b.is_ascii_digit()) { Seg::Num(m.as_str().trim_start_matches('0').to_string()) } else { Seg::Str(m.as_str().to_string()) })
                .collect();
            let split = segs.iter().position(|s| matches!(s, Seg::Str(_))).unwrap_or(segs.len());
            let zero = Seg::Num(String::new());
            let trim = |part: &[Seg]| { let mut p = part.to_vec(); while p.last() == Some(&zero) { p.pop(); } p };
            Some(Self([trim(&segs[..split]), trim(&segs[split..])].concat()))
        }

        /// `<=>`: segment by segment, a missing one read as 0; a number above a string; numbers by value, strings bytewise.
        pub fn cmp(&self, other: &Self) -> Ordering {
            let zero = Seg::Num(String::new());
            for i in 0..self.0.len().max(other.0.len()) {
                let (l, r) = (self.0.get(i).unwrap_or(&zero), other.0.get(i).unwrap_or(&zero));
                let o = match (l, r) {
                    (Seg::Num(a), Seg::Num(b)) => a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
                    (Seg::Str(a), Seg::Str(b)) => a.cmp(b),
                    (Seg::Str(_), Seg::Num(_)) => Ordering::Less,
                    (Seg::Num(_), Seg::Str(_)) => Ordering::Greater,
                };
                if o != Ordering::Equal { return o; }
            }
            Ordering::Equal
        }
    }
}

/// One GET of the latest release: `(status, content type, body)`, or why there is none.
pub type FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<(u16, String, Vec<u8>), String>> + 'a>>;
pub trait Fetch { fn latest<'a>(&'a self) -> FetchFuture<'a>; }

/// Clients::Github's connection: open 5 s, read 10 s; no proxy, no redirect followed, no retry; Rails' two headers.
pub struct LiveFetch { client: reqwest::Client, url: String }

impl LiveFetch {
    pub fn new() -> Self { Self::at(LATEST) }
    /// The same client against another URL (tests: a local server).
    pub fn at(url: &str) -> Self {
        let client = crate::venue::http::client_with(std::time::Duration::from_secs(5), std::time::Duration::from_secs(10), std::time::Duration::from_secs(15));
        Self { client, url: url.to_string() }
    }
}

impl Default for LiveFetch { fn default() -> Self { Self::new() } }

impl Fetch for LiveFetch {
    fn latest<'a>(&'a self) -> FetchFuture<'a> {
        Box::pin(async move {
            // The User-Agent GitHub requires carries no version: an install owes GitHub no fingerprint.
            let mut resp = self.client.get(&self.url).header("Accept", "application/vnd.github+json").header("User-Agent", "Deltabadger")
                .send().await.map_err(|e| if e.is_timeout() { "timeout".to_string() } else { "request failed".to_string() })?;
            let status = resp.status().as_u16();
            let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
            let mut body = Vec::new();
            while let Some(chunk) = resp.chunk().await.map_err(|_| "body unreadable".to_string())? {
                if body.len() + chunk.len() > MAX_BODY { return Err("body over 1 MB".into()); }
                body.extend_from_slice(&chunk);
            }
            Ok((status, content_type, body))
        })
    }
}

/// AppUpdate::CheckJob, `0 */12 * * *`.
pub struct Check<F: Fetch> { pub enabled: bool, pub fetch: F }

pub fn job(env: &dyn Fn(&str) -> Option<String>) -> Box<dyn Job> { Box::new(Check { enabled: enabled(env), fetch: LiveFetch::new() }) }

pub fn spec() -> Spec {
    Spec { name: NAME, scope: None, schedule: Some(Schedule::EveryHours { every: 12, minute: 0 }), jitter: Jitter::NONE, retry: Retry::None, deadline: DEADLINE }
}

impl<F: Fetch> Job for Check<F> {
    fn spec(&self) -> Spec { spec() }
    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> {
        Box::pin(async move {
            if !self.enabled { return Outcome::NothingNew; }
            // Every failure is the same nil to Rails, logged and never raised: the last good answer stays.
            let release = match self.fetch.latest().await {
                Ok((status, content_type, body)) => release_from(status, &content_type, &body).ok_or_else(|| format!("no release in the answer (HTTP {status})")),
                Err(why) => Err(why),
            };
            let release = match release {
                Ok(r) => r,
                Err(why) => { crate::engine::log(&format!("[jobs] Update check skipped: {why}")); return Outcome::NothingNew; }
            };
            let now = cx.clock.now();
            match cx.db.run(move |c, _| store(c, &release, now)).await { Ok(()) => Outcome::Done, Err(e) => Outcome::Failed(e) }
        })
    }
}
