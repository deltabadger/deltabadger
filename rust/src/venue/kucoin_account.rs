//! D-KUCOIN-HF-MODE: account evidence belongs to the captured credential version.
//! Classic (LF) and HF accounts trade; a Unified Trading Account is refused.
use super::http::{self, HttpRequest, Transport};
use super::VenueError;
use crate::engine::model::{CredentialVersion, Produced};
use chrono::{DateTime, Duration, Utc};
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};

pub const MODE: &str = "/api/ua/v2/account/mode";
pub const OPENED: &str = "/api/v1/hf/accounts/opened";
pub const MAX_AGE_SECONDS: i64 = 300;
pub const UTA: &str = "KuCoin Unified Trading Accounts are not supported by this spot adapter.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountMode { ClassicLf, ClassicHf, UnsupportedUta }
impl AccountMode {
    pub fn as_str(self) -> &'static str {
        match self { Self::ClassicLf => "ClassicLf", Self::ClassicHf => "ClassicHf", Self::UnsupportedUta => "UnsupportedUta" }
    }
    /// The account modes this adapter supports: classic and HF. UTA is refused.
    pub fn require_supported(self) -> Result<(), VenueError> {
        match self { Self::ClassicLf | Self::ClassicHf => Ok(()), Self::UnsupportedUta => Err(VenueError::Transient(UTA.into())) }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Detection {
    pub mode: AccountMode,
    at: DateTime<Utc>,
    monotonic: std::time::Instant,
    generation: u64,
}
impl Detection {
    fn fresh(self, now: DateTime<Utc>, generation: u64) -> bool {
        self.generation == generation && now >= self.at && now - self.at < Duration::seconds(MAX_AGE_SECONDS)
            && self.monotonic.elapsed().as_secs() < MAX_AGE_SECONDS as u64
    }
}

struct Slot {
    host: String,
    version: CredentialVersion,
    generation: AtomicU64,
    answer: tokio::sync::Mutex<Option<Detection>>,
}

fn generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

/// Single flight per host/key/version. Only successful typed evidence is cached; no credentials or remote diagnostics.
/// Instances can be isolated in tests; all production consumers use `shared`.
#[derive(Clone, Default)]
pub struct Resolver { slots: Arc<Mutex<Vec<Arc<Slot>>>> }
impl Resolver {
    pub fn shared() -> Self {
        static SHARED: OnceLock<Resolver> = OnceLock::new();
        SHARED.get_or_init(Self::default).clone()
    }
    fn slot(&self, host: &str, version: &CredentialVersion) -> Arc<Slot> {
        let mut slots = self.slots.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(slot) = slots.iter().find(|s| s.host == host && s.version == *version) { return slot.clone(); }
        // Retire replaced credentials, including in-flight evidence. Their handles keep their original producer.
        slots.retain(|s| {
            if s.host == host && s.version.id == version.id {
                s.generation.store(generation(), Ordering::SeqCst);
                false
            } else { true }
        });
        let slot = Arc::new(Slot { host: host.into(), version: version.clone(), generation: AtomicU64::new(generation()), answer: tokio::sync::Mutex::new(None) });
        slots.push(slot.clone());
        slot
    }
    /// Fresh validation invalidates even an in-flight read. No stale result may repopulate the cache afterwards.
    pub fn invalidate(&self, host: &str, key_id: i64) {
        let slots = self.slots.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for slot in slots.iter().filter(|s| s.host == host && s.version.id == key_id) { slot.generation.store(generation(), Ordering::SeqCst); }
    }
    pub async fn resolve<T: Transport>(&self, transport: &T, host: &str, version: &CredentialVersion) -> Produced<Result<Detection, VenueError>> {
        let slot = self.slot(host, version);
        let mut answer = slot.answer.lock().await;
        let cached_generation = slot.generation.load(Ordering::SeqCst);
        let now = transport.now();
        if let Some(cached) = answer.filter(|d| d.fresh(now, cached_generation)) { return Produced::new(Ok(cached), Some(version.clone())); }
        *answer = None;
        let generation = generation();
        slot.generation.store(generation, Ordering::SeqCst);
        let monotonic = std::time::Instant::now();
        let detected = detect(transport, host).await.and_then(|mode| {
            let detection = Detection { mode, at: now, monotonic, generation };
            if !detection.fresh(transport.now(), slot.generation.load(Ordering::SeqCst)) {
                return Err(refusal("account evidence expired or was invalidated during detection"));
            }
            Ok(detection)
        });
        if let Ok(detection) = detected { *answer = Some(detection); }
        Produced::new(detected, Some(version.clone()))
    }
    pub fn is_current<T: Transport>(&self, transport: &T, host: &str, version: &CredentialVersion, detection: Detection) -> bool {
        let slots = self.slots.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        slots.iter().find(|s| s.host == host && s.version == *version)
            .is_some_and(|s| detection.fresh(transport.now(), s.generation.load(Ordering::SeqCst)))
    }
}

pub fn refusal(reason: &str) -> VenueError {
    VenueError::Transient(format!("KuCoin account mode could not be determined: {reason}; order not sent."))
}

async fn read<T: Transport>(transport: &T, host: &str, path: &str) -> Result<serde_json::Value, VenueError> {
    let request = HttpRequest { method: "GET", base: host.into(), path: path.into(), query: vec![], body: None, not_after: None };
    let reply = transport.send(&request).await.map_err(|_| refusal(&format!("{path}: transport failure")))?;
    // Never expose venue-controlled text (including unknown mode strings) in a preflight diagnostic.
    if !(200..300).contains(&reply.status) { return Err(refusal(&format!("{path}: HTTP {}", reply.status))); }
    let body = http::decode_json(&reply.body).map_err(|_| refusal(&format!("{path}: unreadable response")))?;
    if body["code"].as_str() != Some("200000") { return Err(refusal(&format!("{path}: missing or unsuccessful result code"))); }
    Ok(body)
}

/// The mode read now, past any cache (UTA r2.1: a unit is certified only on the mode detected at its end).
pub(crate) async fn detect<T: Transport>(transport: &T, host: &str) -> Result<AccountMode, VenueError> {
    let mode = read(transport, host, MODE).await?;
    match mode["data"]["selfAccountMode"].as_str() {
        Some("UNIFIED") => Ok(AccountMode::UnsupportedUta),
        Some("CLASSIC") => match read(transport, host, OPENED).await?["data"].as_bool() {
            Some(false) => Ok(AccountMode::ClassicLf), Some(true) => Ok(AccountMode::ClassicHf),
            None => Err(refusal("opened response has no boolean data")),
        },
        _ => Err(refusal("missing or unknown selfAccountMode")),
    }
}
