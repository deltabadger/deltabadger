//! The mail sender: a background service of the supervisor, beside the engine. It reads the markers the engine left on
//! bots (engine::notice), renders each mail from the rows as they are now, delivers it, and clears the marker only once
//! the mail server has accepted the message.
//!
//! - It looks at start, on every wake (an engine event), when a retry is due, and at least every `poll`.
//! - **It never holds the engine's thread.** Every SQLite statement of the sender runs on tokio's blocking pool, on the
//!   sender's own connection (as the web's `App::db` does): a locked database makes a pool thread wait, not the runtime.
//!   A look reads the markers of at most `batch` bots and goes round; between two mails the sender yields.
//! - With no usable SMTP settings (smtp::Settings::current is Err) it says so once, with the reason, and sends nothing:
//!   the markers stay, and wait out their week. Settings are read again for every mail, so mail configured later goes out.
//! - At least once: a mail whose delivery was cut off after the server had it is sent again. Never zero, while the
//!   marker is under seven days old.
//! - Anything that fails is tried again after 3, 18, 83 and 258 s (ApplicationMailDeliveryJob's waits), then hourly: a
//!   delivery, a mail that could not be built for now, and a clear the database or `eligibility::guard` refused. A
//!   marker whose mail was accepted but whose clear failed is remembered: the retry only clears, it does not send again
//!   (a restart forgets that, and sends once more). A mail that could not be rendered is logged once a day, not at
//!   every hourly retry.
//! - A marker seven days old is cleared unsent, delivered or not, rendered or not: long enough to outlast a weekend's
//!   SMTP outage or broken configuration (a stop notice never recurs), short enough that the news is not stale. One whose
//!   mail cannot be sent as it stands (its bot's user is gone; a header value holds a control character) is cleared at once.
//! - A stop drops whatever is in hand, a delivery included, and records nothing: the marker is still there at the next start.
//! - Nothing here can fail a tick or end the process: an error is logged and tried again.
//! - A log line names a bot, a mail and a `smtp::Failure`: never an address, a credential, or a word the server wrote.
use super::{render, smtp};
use crate::crypto::Cipher;
use crate::engine::notice::{self, Pending};
use crate::engine::{log, Clock};
use rusqlite::Connection;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc::UnboundedReceiver, watch};
use tokio::time::Instant;

/// How long a marker waits for its mail to be delivered (or rendered) before it is given up.
const MAX_AGE_HOURS: i64 = 7 * 24;
/// How often a mail that keeps failing to render is logged.
const RENDER_LOG_EVERY: Duration = Duration::from_secs(24 * 3600);

pub struct Sender<C: Clock> {
    db: Arc<Mutex<Connection>>,
    cipher: Arc<Cipher>,
    env: Arc<smtp::Env>,
    urls: Arc<render::Urls>,
    pub clock: C,
    /// How often the markers are read when nothing wakes the sender sooner.
    pub poll: Duration,
    /// The wait after the 1st, 2nd, … failure for one marker; the last one repeats.
    pub waits: Vec<Duration>,
    /// How many bots' markers one look reads.
    pub batch: usize,
    /// Tests only: see smtp::Settings::trust_root.
    pub trust_root: Option<Vec<u8>>,
}

/// Per marker: failures so far, when to look at it next, whether its mail has already been accepted (then only the
/// clear is still owed), and when a failure to render it was last logged. Keyed by the marker itself, so a marker raised
/// again starts afresh.
type Attempts = HashMap<Pending, (usize, Instant, bool, Option<Instant>)>;

/// What the blocking pool hands back for one marker.
enum Prepared {
    /// A mail and where to send it: the settings, the sender, the recipient, the encoded message.
    Ready(Box<(smtp::Settings, String, String, String)>),
    /// No usable SMTP settings, and why.
    NotConfigured(String),
    /// No mail can come of this marker as the rows stand: it is cleared, and the reason logged.
    Unsendable(String),
    /// Not now: tried again like a failed delivery.
    Later(String),
}

impl<C: Clock> Sender<C> {
    /// `db`: this service's own connection (store::configure'd), never the engine's.
    pub fn new(db: Connection, cipher: Cipher, env: &dyn Fn(&str) -> Option<String>, clock: C) -> Self {
        Self { db: Arc::new(Mutex::new(db)), cipher: Arc::new(cipher), env: Arc::new(smtp::Env::read(env)), urls: Arc::new(render::Urls::from_env(env)), clock,
               poll: Duration::from_secs(5), waits: [3, 18, 83, 258, 3600].map(Duration::from_secs).to_vec(), batch: 20, trust_root: None }
    }

    /// The service's future. Returns Ok once `stop` turns true (the supervisor's one stop signal, `Shutdown::subscribe()`),
    /// dropping whatever is in hand; it never returns before that. `wake`: any channel of engine events; every message
    /// means "look now", its content is not read.
    pub async fn run<T>(self, mut stop: watch::Receiver<bool>, wake: Option<UnboundedReceiver<T>>) -> Result<(), String> {
        tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => Ok(()),
            never = self.work(wake) => match never {},
        }
    }

    /// Database work, off the runtime's thread.
    async fn db<R: Send + 'static>(&self, work: impl FnOnce(&Connection, &Cipher) -> R + Send + 'static) -> Result<R, String> {
        let (db, cipher) = (self.db.clone(), self.cipher.clone());
        tokio::task::spawn_blocking(move || work(&db.lock().unwrap_or_else(PoisonError::into_inner), &cipher)).await.map_err(|e| e.to_string())
    }

    async fn work<T>(&self, mut wake: Option<UnboundedReceiver<T>>) -> Infallible {
        let mut attempts = Attempts::new();
        let mut said: Option<String> = None; // the "not configured" reason last logged
        let mut after = 0; // the bot id the next look starts above
        let (env, trust) = (self.env.clone(), self.trust_root.clone());
        if let Ok(Err(reason)) = self.db(move |c, cipher| configuration(c, cipher, &env, trust).map(|_| ())).await { self.not_configured(&reason, &mut said); }
        loop {
            let batch = self.batch;
            let wait = match self.db(move |c, _| notice::pending(c, after, batch)).await {
                Ok(Ok((markers, next))) => {
                    after = next;
                    let now = Instant::now();
                    attempts.retain(|_, (_, due, _, _)| *due + Duration::from_secs(3600) > now); // nobody else clears a marker; this only bounds the map
                    let mut wait = self.poll;
                    for marker in markers {
                        match attempts.get(&marker) {
                            Some((_, due, _, _)) if *due > Instant::now() => wait = wait.min(*due - Instant::now()),
                            _ => { self.attempt(marker, &mut attempts, &mut said).await; tokio::task::yield_now().await; }
                        }
                    }
                    // More bots to read: go on at once (after a yield); else rest.
                    if next != 0 { Duration::ZERO } else { wait }
                }
                Ok(Err(e)) => { log(&format!("[mail] the markers could not be read: {e:?}; trying again in {} s", self.poll.as_secs())); self.poll }
                Err(e) => { log(&format!("[mail] the markers could not be read: {e}; trying again in {} s", self.poll.as_secs())); self.poll }
            };
            match &mut wake {
                Some(events) => tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    event = events.recv() => if event.is_none() { wake = None }, // the engine is gone: the timer still runs
                },
                None => tokio::time::sleep(wait).await,
            }
        }
    }

    fn not_configured(&self, reason: &str, said: &mut Option<String>) {
        if said.as_deref() == Some(reason) { return; }
        log(&format!("[mail] mail is not configured ({reason}): nothing is sent; what a bot is owed stays on its row for seven days"));
        *said = Some(reason.to_string());
    }

    /// Tried again, by the same schedule whatever failed.
    fn later(&self, marker: Pending, tried: usize, accepted: bool, why: &str, attempts: &mut Attempts) {
        let wait = self.wait(tried);
        log(&format!("[mail] {} for bot {}: {why}; trying again in {} s", marker.notice.mail(), marker.bot_id, wait.as_secs()));
        attempts.insert(marker, (tried + 1, Instant::now() + wait, accepted, None));
    }

    /// A mail that could not be rendered: the same waits, but logged once a day, not at every retry.
    fn unrendered(&self, marker: Pending, tried: usize, why: &str, attempts: &mut Attempts) {
        let wait = self.wait(tried);
        let last = attempts.get(&marker).and_then(|(.., logged)| *logged);
        let logged = match last {
            Some(at) if at.elapsed() < RENDER_LOG_EVERY => at,
            _ => {
                log(&format!("[mail] {} for bot {}: not sent: {why}; trying again in {} s (said once a day)", marker.notice.mail(), marker.bot_id, wait.as_secs()));
                Instant::now()
            }
        };
        attempts.insert(marker, (tried + 1, Instant::now() + wait, false, Some(logged)));
    }

    fn wait(&self, tried: usize) -> Duration { self.waits.get(tried).or(self.waits.last()).copied().unwrap_or(self.poll) }

    async fn attempt(&self, marker: Pending, attempts: &mut Attempts, said: &mut Option<String>) {
        let (tried, accepted) = attempts.get(&marker).map_or((0, false), |(n, _, accepted, _)| (*n, *accepted));
        let now = self.clock.now();
        let raised = match marker.raised_at() { Ok(at)=>at, Err(e)=>return self.later(marker, tried, false, &format!("unreadable timestamp: {e:?}"), attempts) };
        let (outcome, accepted) = if accepted {
            ("sent".to_string(), true)
        } else if now - raised >= chrono::Duration::hours(MAX_AGE_HOURS) {
            ("given up: raised seven days ago or more".to_string(), false)
        } else {
            let (m, env, urls, trust) = (marker.clone(), self.env.clone(), self.urls.clone(), self.trust_root.clone());
            match self.db(move |c, cipher| prepare(c, cipher, &m, &env, &urls, trust, now)).await {
                Err(e) => return self.later(marker, tried, false, &format!("not sent: {e}"), attempts),
                Ok(Prepared::Later(why)) => return self.unrendered(marker, tried, &why, attempts),
                Ok(Prepared::NotConfigured(reason)) => {
                    // Said once. The marker waits, quietly, for settings or for its seven days to end.
                    self.not_configured(&reason, said);
                    attempts.insert(marker, (tried, Instant::now() + self.poll, false, None));
                    return;
                }
                Ok(Prepared::Unsendable(why)) => (format!("not sent: {why}"), false),
                Ok(Prepared::Ready(ready)) => {
                    let (settings, from, to, message) = *ready;
                    match smtp::deliver(&settings, &from, &to, &message).await {
                        Ok(()) => ("sent".to_string(), true),
                        Err(failure) => return self.later(marker, tried, false, &format!("not sent: {failure}"), attempts),
                    }
                }
            }
        };
        let m = marker.clone();
        match self.db(move |c, cipher| notice::clear(c, cipher, &m)).await {
            Ok(Ok(_)) => {
                log(&format!("[mail] {} for bot {}: {outcome}", marker.notice.mail(), marker.bot_id));
                attempts.remove(&marker);
            }
            // A failed clear is a failed attempt: the same waits. `accepted` keeps the retry from sending again.
            Ok(Err(e)) => self.later(marker, tried, accepted, &format!("{outcome}, but its marker could not be cleared ({e:?})"), attempts),
            Err(e) => self.later(marker, tried, accepted, &format!("{outcome}, but its marker could not be cleared ({e})"), attempts),
        }
    }
}

/// The sender address and where to deliver, read for every mail, as Rails' interceptor reads them: a change made in
/// Settings applies to the next mail. Err: why mail is not configured.
fn configuration(c: &Connection, cipher: &Cipher, env: &smtp::Env, trust_root: Option<Vec<u8>>) -> Result<(String, smtp::Settings), String> {
    let unreadable = std::cell::RefCell::new(None);
    let config = |key: &str| match crate::web::auth::app_config(c, cipher, key) {
        Ok(value) => value,
        Err(_) => { unreadable.borrow_mut().get_or_insert_with(|| key.to_string()); None }
    };
    let from = smtp::notifications_sender(env, &config);
    let settings = smtp::Settings::current(env, &config);
    if let Some(key) = unreadable.into_inner() { return Err(format!("app_configs[{key}] cannot be read with this install's key")); }
    let mut settings = settings?;
    settings.trust_root = trust_root;
    Ok((from, settings))
}

/// Everything a delivery needs, from the database, in one visit to the blocking pool.
fn prepare(c: &Connection, cipher: &Cipher, marker: &Pending, env: &smtp::Env, urls: &render::Urls, trust_root: Option<Vec<u8>>, now: chrono::DateTime<chrono::Utc>) -> Prepared {
    let (from, settings) = match configuration(c, cipher, env, trust_root) {
        Ok(found) => found,
        Err(reason) => return Prepared::NotConfigured(reason),
    };
    let message = match render::for_notice(c, marker.bot_id, &marker.notice, &from, urls) {
        Ok(Some(message)) => message,
        Ok(None) => return Prepared::Unsendable("its bot's user, venue or quote asset is gone".into()),
        Err(e) => return Prepared::Later(format!("it could not be rendered ({e:?})")),
    };
    let id = format!("<{}@{}>", uuid::Uuid::new_v4().simple(), settings.domain);
    match message.encode(now, &id) {
        Ok(encoded) => Prepared::Ready(Box::new((settings, message.from, message.to, encoded))),
        // Never repaired or cut short: a header that would carry a line break is not sent at all.
        Err(why) => Prepared::Unsendable(why.to_string()),
    }
}
