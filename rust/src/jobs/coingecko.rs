//! Clients::Coingecko (app/models/clients/coingecko.rb) over a Transport: the plan ping that picks the host and the key
//! header (:26-44), the per-plan rate limit (:318-329), and Client#with_rescue's outcomes as data_api::ApiError, so a 429
//! is `rate_limited()` as it is on data-api. One client per key; every job and request of the process shares the key's
//! throttle.
use super::data_api::{ApiError, MAX_REFERENCE_BODY};
use crate::venue::http::{self, HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::time::Instant;

pub const PRO: &str = "https://pro-api.coingecko.com";
pub const DEMO: &str = "https://api.coingecko.com";
const PATH: &str = "/api/v3";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plan { Pro, Demo }

impl Plan {
    pub fn base(self) -> &'static str { match self { Self::Pro => PRO, Self::Demo => DEMO } }
    /// `rate = plan == :pro ? 250 : 30` requests per 60 s.
    pub fn per_minute(self) -> usize { match self { Self::Pro => 250, Self::Demo => 30 } }
}

/// The header a request to `base` carries the key in: `x-cg-pro-api-key` on the pro host, else `x-cg-demo-api-key`.
pub fn header_for(base: &str) -> &'static str { if base == PRO { "x-cg-pro-api-key" } else { "x-cg-demo-api-key" } }

/// Faraday's `rate_limit` middleware: one key's requests of the last 60 s, whatever host or plan they went to; a request
/// waits while the window holds its limit (the lower of the caller's and `cap`). On tokio's clock, so a test with a paused
/// clock sees the wait.
pub struct Throttle { cap: usize, sent: Mutex<VecDeque<Instant>> }

impl Throttle {
    pub fn new(cap: usize) -> Self { Self { cap, sent: Mutex::new(VecDeque::new()) } }

    /// Waits until the window holds fewer than `limit` requests, then takes a slot.
    pub async fn acquire(&self, limit: usize) {
        let limit = limit.min(self.cap);
        loop {
            let wait = {
                let mut sent = self.sent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let now = Instant::now();
                while sent.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60)) { sent.pop_front(); }
                if sent.len() < limit { sent.push_back(now); return; }
                Duration::from_secs(60) - now.duration_since(sent[0])
            };
            tokio::time::sleep(wait).await;
        }
    }
}

/// The process's throttle for a key: one request history the plan ping and every call share. ponytail: one map entry per
/// key ever used in the process (a handful).
type Throttles = Mutex<HashMap<String, Arc<Throttle>>>;

pub fn throttle(key: &str) -> Arc<Throttle> {
    static ALL: OnceLock<Throttles> = OnceLock::new();
    let mut all = ALL.get_or_init(Default::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    all.entry(key.to_string()).or_insert_with(|| Arc::new(Throttle::new(usize::MAX))).clone()
}

/// The live transport: the key in the header its host takes, JSON, bounded bodies.
pub struct LiveTransport { client: reqwest::Client, key: String }

impl LiveTransport {
    pub fn new(key: &str) -> Self { Self { client: http::client(), key: key.into() } }
}

impl Transport for LiveTransport {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> { self.send_limited(r, MAX_REFERENCE_BODY).await }
    async fn send_limited(&self, r: &HttpRequest, limit: usize) -> Result<HttpResponse, TransportError> {
        let mut url = reqwest::Url::parse(&format!("{}{}", r.base, r.path)).map_err(|e| TransportError::Permanent(e.to_string()))?;
        if !r.query.is_empty() { url.query_pairs_mut().extend_pairs(&r.query); }
        let resp = self.client.get(url).header(header_for(&r.base), &self.key).header("Accept", "application/json")
            .send().await.map_err(http::classify)?;
        http::read_limited(resp, limit.min(MAX_REFERENCE_BODY)).await
    }
    /// The key never reaches a log line.
    fn redact_diagnostic(&self, text: &str) -> String { if self.key.is_empty() { text.into() } else { text.replace(&self.key, "[key]") } }
}

pub struct CoinGecko<T: Transport> {
    key: String,
    transport: T,
    plan: tokio::sync::OnceCell<Plan>,
    /// Set for tests; else the process's throttle for the key and plan.
    throttle: Option<Arc<Throttle>>,
    /// D-CG-429-STOPS-PRICES: a 429 met by this client's historical requests (jobs::market::Feed); the client is the run's.
    limited: std::sync::atomic::AtomicBool,
}

impl CoinGecko<LiveTransport> {
    pub fn live(key: &str) -> Self { Self::new(key, LiveTransport::new(key)) }
}

impl<T: Transport> CoinGecko<T> {
    pub fn new(key: &str, transport: T) -> Self { Self { key: key.into(), transport, plan: Default::default(), throttle: None, limited: Default::default() } }
    pub fn with_throttle(mut self, t: Arc<Throttle>) -> Self { self.throttle = Some(t); self }
    pub fn transport(&self) -> &T { &self.transport }
    /// Whether a key is set (Coingecko's `@api_key.blank?` guard is the caller's).
    pub fn has_key(&self) -> bool { !self.key.trim().is_empty() }
    /// D-CG-429-STOPS-PRICES: whether this client's price requests met a 429.
    pub fn limited(&self) -> bool { self.limited.load(std::sync::atomic::Ordering::Relaxed) }
    pub fn limit(&self) { self.limited.store(true, std::sync::atomic::Ordering::Relaxed) }

    async fn slot(&self, limit: usize) {
        match &self.throttle { Some(t) => t.acquire(limit).await, None => throttle(&self.key).acquire(limit).await }
    }

    fn request(base: &str, path: &str, query: &[(&'static str, String)]) -> HttpRequest {
        HttpRequest { method: "GET", base: base.into(), path: format!("{PATH}{path}"), query: query.to_vec(), body: None, not_after: None }
    }

    /// Clients::Coingecko.detect_plan (D-CG-PLAN-PING): the pro ping answers 200 → pro; 429 → the call is rate limited
    /// (never "demo"); any other answer → demo, as Rails; no answer → transient (Rails memoises "demo" for the process).
    /// Once per client.
    pub async fn plan(&self) -> Result<Plan, ApiError> {
        self.plan.get_or_try_init(|| async {
            // The ping counts against the key's quota too: it takes a slot of the demo pace (the stricter one), as the plan
            // is not yet known.
            self.slot(Plan::Demo.per_minute()).await;
            match self.transport.send_limited(&Self::request(PRO, "/ping", &[]), 64 * 1024).await {
                Ok(r) if r.status == 200 => Ok(Plan::Pro),
                Ok(r) if r.status == 429 => Err(ApiError::Failed { status: Some(429), message: "CoinGecko plan check: HTTP 429".into() }),
                Ok(_) => Ok(Plan::Demo),
                Err(TransportError::Permanent(m)) => Err(ApiError::Failed { status: None, message: format!("CoinGecko plan check: {}", self.transport.redact_diagnostic(&m)) }),
                Err(TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(ApiError::Transient(format!("CoinGecko plan check: {}", self.transport.redact_diagnostic(&m)))),
            }
        }).await.copied()
    }

    /// One GET of `path` under /api/v3, after the plan and a throttle slot. Any answer but 2xx is `Failed` with its status.
    pub async fn get(&self, path: &str, query: &[(&'static str, String)]) -> Result<Value, ApiError> {
        let plan = self.plan().await?;
        self.slot(plan.per_minute()).await;
        let req = Self::request(plan.base(), path, query);
        let resp = match self.transport.send_limited(&req, MAX_REFERENCE_BODY).await {
            Ok(r) => r,
            Err(TransportError::Permanent(m)) => return Err(ApiError::Failed { status: None, message: self.transport.redact_diagnostic(&m) }),
            Err(TransportError::NotSent(m) | TransportError::MaybeSent(m)) => return Err(ApiError::Transient(self.transport.redact_diagnostic(&m))),
        };
        if !(200..300).contains(&resp.status) { return Err(ApiError::Failed { status: Some(resp.status), message: format!("CoinGecko {path}: HTTP {}", resp.status) }); }
        if resp.body.trim().is_empty() { return Ok(Value::Null); }
        let status = resp.status;
        tokio::task::spawn_blocking(move || serde_json::from_str::<Value>(&resp.body))
            .await
            .map_err(|e| ApiError::Failed { status: None, message: format!("the blocking pool lost the parse: {e}") })?
            .map_err(|_| ApiError::Failed { status: Some(status), message: format!("CoinGecko {path}: unreadable response (HTTP {status})") })
    }

    /// Clients::Coingecko#coins_list_with_market_data (:76-104), one page: the params Faraday sends, sorted as it sorts them.
    pub async fn markets(&self, ids: Option<&[String]>, category: Option<&str>, per_page: usize, page: usize) -> Result<Value, ApiError> {
        let mut q: Vec<(&'static str, String)> = vec![];
        if let Some(c) = category { q.push(("category", c.into())); }
        if let Some(ids) = ids.filter(|i| !i.is_empty()) { q.push(("ids", ids.join(","))); }
        q.extend([("locale", "en".into()), ("order", "market_cap_desc".into()), ("page", page.to_string()), ("per_page", per_page.to_string()),
                  ("precision", "full".into()), ("sparkline", "false".into()), ("vs_currency", "usd".into())]);
        self.get("/coins/markets", &q).await
    }

    /// Clients::Coingecko#coin_data_by_id (:117-140) as Coingecko#get_coin_data_by_id asks it: market data only.
    pub async fn coin(&self, id: &str) -> Result<Value, ApiError> {
        let q = [("community_data", "false"), ("developer_data", "false"), ("localization", "false"), ("market_data", "true"),
                 ("sparkline", "false"), ("tickers", "false")].map(|(k, v)| (k, v.to_string()));
        self.get(&format!("/coins/{id}"), &q).await
    }

    /// Clients::Coingecko#exchange_tickers_by_id (:251-272) as Coingecko#get_exchange_tickers_by_id asks it: one page of a
    /// venue's tickers by base asset (coin_ids nil is compacted away; the two false flags are sent).
    pub async fn exchange_tickers(&self, id: &str, page: usize) -> Result<Value, ApiError> {
        let q = [("depth", "false".to_string()), ("include_exchange_logo", "false".into()), ("order", "base_asset".into()), ("page", page.to_string())];
        self.get(&format!("/exchanges/{id}/tickers"), &q).await
    }
}

impl<T: Transport> CoinGecko<T> {
    /// Clients::Coingecko#coin_historical_chart_data_within_time_range_by_id (:210-231): `interval=daily`,
    /// `precision=full`, `from`/`to` as Unix seconds; the params sorted as Faraday sends them.
    pub async fn market_chart_range(&self, id: &str, vs_currency: &str, from: i64, to: i64) -> Result<Value, ApiError> {
        let q = [("from", from.to_string()), ("interval", "daily".into()), ("precision", "full".into()), ("to", to.to_string()), ("vs_currency", vs_currency.into())];
        self.get(&format!("/coins/{id}/market_chart/range"), &q).await
    }

    /// Clients::Coingecko#coin_price_by_ids (:150-174) with its defaults: the ids and currencies comma-joined, the four
    /// include flags false, `precision=full`. The answer as CoinGecko gives it (`{id: {cur: p}}`).
    pub async fn simple_price(&self, ids: &[String], vs_currencies: &str) -> Result<Value, ApiError> {
        let q = [("ids", ids.join(",")), ("include_24hr_change", "false".into()), ("include_24hr_vol", "false".into()),
                 ("include_last_updated_at", "false".into()), ("include_market_cap", "false".into()), ("precision", "full".into()),
                 ("vs_currencies", vs_currencies.into())];
        self.get("/simple/price", &q).await
    }

    /// Clients::Coingecko#exchange_rates (:277-284): the answer's `rates`.
    pub async fn exchange_rates(&self) -> Result<Value, ApiError> { Ok(self.get("/exchange_rates", &[]).await?["rates"].clone()) }
}

pub type Fut<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, ApiError>> + 'a>>;

/// What the jobs ask CoinGecko, object-safe so a job holds live or scripted clients alike.
pub trait Api {
    fn markets<'a>(&'a self, ids: Option<&'a [String]>, category: Option<&'a str>, per_page: usize, page: usize) -> Fut<'a>;
    fn coin<'a>(&'a self, id: &'a str) -> Fut<'a>;
    fn exchange_tickers<'a>(&'a self, id: &'a str, page: usize) -> Fut<'a>;
}

impl<T: Transport> Api for CoinGecko<T> {
    fn markets<'a>(&'a self, ids: Option<&'a [String]>, category: Option<&'a str>, per_page: usize, page: usize) -> Fut<'a> {
        Box::pin(CoinGecko::markets(self, ids, category, per_page, page))
    }
    fn coin<'a>(&'a self, id: &'a str) -> Fut<'a> { Box::pin(CoinGecko::coin(self, id)) }
    fn exchange_tickers<'a>(&'a self, id: &'a str, page: usize) -> Fut<'a> { Box::pin(CoinGecko::exchange_tickers(self, id, page)) }
}

/// Why a venue's crawl found nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum CrawlError {
    /// A 429 (the plan ping's or a page's): D-CG-429-NO-MORE-CRAWLS, the run asks no further venue.
    RateLimited(String),
    /// Any other failure: this venue's discovery is discarded, the next venue still crawls.
    Failed(String),
}

/// Coingecko#get_exchange_tickers_by_id (coingecko.rb:38-54): pages of 100 by base asset until a short page, at most 50;
/// any page's failure, or a 50th full page (Rails raises "Too many attempts"), discards every page read. Paced by the key's
/// throttle (D-CG-THROTTLE), not `sleep 3`. A blank key fails before any request.
pub async fn crawl(api: &dyn Api, key: &str, id: &str) -> Result<Vec<Value>, CrawlError> {
    if key.trim().is_empty() { return Err(CrawlError::Failed("CoinGecko API key not configured".into())); }
    let mut all = vec![];
    for page in 1..=50 {
        let body = match api.exchange_tickers(id, page).await {
            Ok(b) => b,
            Err(e) if e.rate_limited() => return Err(CrawlError::RateLimited(e.message())),
            Err(e) => return Err(CrawlError::Failed(e.message())),
        };
        let Some(tickers) = body.get("tickers").and_then(Value::as_array) else {
            return Err(CrawlError::Failed(format!("CoinGecko exchanges/{id}/tickers page {page}: no tickers list")));
        };
        let short = tickers.len() < 100;
        all.extend(tickers.iter().cloned());
        if short { return Ok(all); }
    }
    Err(CrawlError::Failed(format!("Too many attempts to get {id} tickers by id from CoinGecko. Adjust the number of pages in the loop if needed.")))
}

/// A client for a key: `live` in production; a test hands scripted ones.
pub type Clients = std::rc::Rc<dyn Fn(&str) -> Box<dyn Api>>;

pub fn live_clients() -> Clients { std::rc::Rc::new(|key: &str| Box::new(CoinGecko::live(key)) as Box<dyn Api>) }

/// The key check of the tracker and wizard (`validate_coingecko_api_key`: get_top_coins_by_market_cap(limit: 5), one
/// `coins/markets` page of 250): valid when CoinGecko answers a list. One deadline over the whole check (plan ping,
/// throttle wait, request). Never cached (D-CG-KEY-CHECK-UNCACHED); no answer is "invalid", not an error page
/// (D-CG-KEY-CHECK-NO-500).
pub const CHECK_DEADLINE: Duration = Duration::from_secs(10);

pub async fn check<T: Transport>(client: &CoinGecko<T>) -> bool {
    if client.key.trim().is_empty() { return false; }
    matches!(tokio::time::timeout(CHECK_DEADLINE, client.markets(None, None, 250, 1)).await, Ok(Ok(Value::Array(_))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venue::http::ScriptedTransport;
    use serde_json::json;

    fn rt() -> tokio::runtime::Runtime { tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build().unwrap() }

    #[test]
    fn the_ping_picks_the_host_and_a_429_is_never_demo() {
        rt().block_on(async {
            let t = ScriptedTransport::default();
            t.reply("GET /api/v3/ping", 401, json!({})).reply("GET /api/v3/coins/markets", 200, json!([]));
            let cg = CoinGecko::new("k", t.clone()).with_throttle(Arc::new(Throttle::new(30)));
            assert_eq!(cg.markets(None, None, 250, 1).await, Ok(json!([])));
            let r = t.requests();
            assert_eq!((r[0].base.as_str(), r[1].base.as_str()), (PRO, DEMO));
            assert_eq!(header_for(&r[1].base), "x-cg-demo-api-key");

            let t = ScriptedTransport::default();
            t.reply("GET /api/v3/ping", 429, json!({}));
            let cg = CoinGecko::new("k", t.clone()).with_throttle(Arc::new(Throttle::new(30)));
            assert!(cg.markets(None, None, 250, 1).await.unwrap_err().rate_limited());
            assert_eq!(t.requests().len(), 1, "nothing asked after a rate-limited ping");

            let t = ScriptedTransport::default();
            t.network("GET /api/v3/ping", "pre_send", "down");
            let cg = CoinGecko::new("k", t).with_throttle(Arc::new(Throttle::new(30)));
            assert!(matches!(cg.plan().await, Err(ApiError::Transient(_))));
        });
    }

    #[test]
    fn the_throttle_holds_the_31st_request_of_a_minute() {
        rt().block_on(async {
            let th = Throttle::new(30);
            let start = Instant::now();
            for _ in 0..30 { th.acquire(30).await; }
            assert_eq!(start.elapsed(), Duration::ZERO);
            th.acquire(30).await;
            assert_eq!(start.elapsed(), Duration::from_secs(60));
        });
    }

    /// The ping and the calls of every client of one key share one minute: a pro key that has spent 30 requests makes
    /// a new client's ping wait, and the key's 251st request of a minute waits whatever asked it.
    #[test]
    fn one_key_shares_one_window_across_clients_and_the_ping() {
        rt().block_on(async {
            let t = ScriptedTransport::default();
            t.reply("GET /api/v3/ping", 200, json!({})).reply("GET /api/v3/coins/markets", 200, json!([]));
            let key = "shared-window-test-key";
            let start = Instant::now();
            let first = CoinGecko::new(key, t.clone());
            for _ in 0..29 { first.markets(None, None, 250, 1).await.unwrap(); }
            assert_eq!((t.requests().len(), start.elapsed()), (30, Duration::ZERO), "ping + 29 calls at once");
            CoinGecko::new(key, t.clone()).plan().await.unwrap();
            assert_eq!(start.elapsed(), Duration::from_secs(60), "a second client's ping waits for the key's window");
            // The window now holds that ping; 249 pro calls fill it to 250 at once.
            for _ in 0..249 { first.markets(None, None, 250, 1).await.unwrap(); }
            assert_eq!((t.requests().len(), start.elapsed()), (280, Duration::from_secs(60)));
            first.markets(None, None, 250, 1).await.unwrap();
            assert_eq!(start.elapsed(), Duration::from_secs(120), "the 251st request of the minute waits");
        });
    }

    #[test]
    fn the_check_is_bounded_and_needs_a_list() {
        rt().block_on(async {
            let ok = ScriptedTransport::default();
            ok.reply("GET /api/v3/ping", 200, json!({})).reply("GET /api/v3/coins/markets", 200, json!([{"id": "bitcoin"}]));
            assert!(check(&CoinGecko::new("k", ok).with_throttle(Arc::new(Throttle::new(250)))).await);
            let bad = ScriptedTransport::default();
            bad.reply("GET /api/v3/ping", 400, json!({})).reply("GET /api/v3/coins/markets", 401, json!({"status": {"error_code": 10002}}));
            assert!(!check(&CoinGecko::new("k", bad).with_throttle(Arc::new(Throttle::new(30)))).await);
            // A full throttle: the wait counts against the deadline.
            let full = Arc::new(Throttle::new(1));
            full.acquire(1).await;
            let t = ScriptedTransport::default();
            t.reply("GET /api/v3/ping", 200, json!({})).reply("GET /api/v3/coins/markets", 200, json!([]));
            let start = Instant::now();
            assert!(!check(&CoinGecko::new("k", t).with_throttle(full)).await);
            assert_eq!(start.elapsed(), CHECK_DEADLINE);
            assert!(!check(&CoinGecko::new(" ", ScriptedTransport::default())).await, "a blank key asks nothing");
        });
    }
}
