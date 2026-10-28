//! G-LIVE v1 is append-only JSONL: a `header`, paired `request`/`answer` records, then `run`.
//! Each row has `format: "deltabadger.withdrawal.g-live", version: 1, at: RFC3339`.
//! Requests contain method/path only (no headers, bodies or queries). Answers retain HTTP status
//! and the raw JSON structure after recursive credential/signature/URL scrubbing; non-JSON is
//! capped safe text. Transport failures have a classification, never an invented HTTP answer.
//! Each line is synced before proceeding. A missing answer/run after a crash means unresolved,
//! never permission to send again. An existing output file is refused; restart uses a new file.
//! `max_amount` is a total budget in asset units, including the conservative fee reserve.
//! `--destination` is JSON with address, network, tag, key (optional fields absent or null).
//! Example (run from the install directory with its normal environment):
//! record_withdrawal --scenario external-1 --user 1 --exchange 1 --asset 1 \
//!   --max-amount 0.001 --destination '{"address":"...","network":"BTC"}' --output /private/live.jsonl
//! Resolve an unconfirmed run using `deltabadger resolve-withdrawal --recorder external-1
//! --sent VENUE_ID` or `--not-sent`, with independent venue evidence.
use deltabadger::{
    crypto::{Cipher, EncryptionKeys},
    engine::{model, withdrawal as w, SystemClock},
    lease, store,
    venue::{
        self,
        http::{HttpRequest, HttpResponse, ReqwestTransport, Transport, TransportError},
        Venue,
    },
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
};

struct Options {
    spec: w::RecorderSpec,
    output: PathBuf,
}
impl Options {
    fn parse(args: &[String]) -> Result<Self, String> {
        let usage = "usage: record_withdrawal --scenario ID --max-amount A --destination JSON --user ID --exchange ID --asset ID --output FILE";
        if !args.len().is_multiple_of(2) {
            return Err(usage.into());
        }
        let mut fields = BTreeMap::new();
        for pair in args.chunks_exact(2) {
            if ![
                "--scenario",
                "--max-amount",
                "--destination",
                "--user",
                "--exchange",
                "--asset",
                "--output",
            ]
            .contains(&pair[0].as_str())
                || fields.insert(pair[0].as_str(), pair[1].as_str()).is_some()
            {
                return Err(usage.into());
            }
        }
        let get = |key| {
            fields
                .get(key)
                .copied()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| usage.to_string())
        };
        let id = |key| {
            get(key)?
                .parse::<i64>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| usage.to_string())
        };
        let destination: Value = serde_json::from_str(get("--destination")?)
            .map_err(|_| "destination must be a JSON object")?;
        let object = destination
            .as_object()
            .ok_or("destination must be a JSON object")?;
        if object.iter().any(|(k, v)| {
            !["address", "network", "tag", "key"].contains(&k.as_str())
                || !(v.is_string() || v.is_null())
        }) {
            return Err("destination accepts address, network, tag and key only".into());
        }
        let address = destination["address"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or("destination needs an address")?;
        let spec = w::RecorderSpec {
            scenario: get("--scenario")?.into(),
            max_amount: get("--max-amount")?.into(),
            user_id: id("--user")?,
            exchange_id: id("--exchange")?,
            asset_id: id("--asset")?,
            destination: venue::withdrawal::Stored::new(
                address,
                destination["network"].as_str(),
                destination["tag"].as_str(),
                destination["key"].as_str(),
            ),
        };
        w::validate_recorder(&spec).map_err(|_| "invalid scenario, amount or account ids")?;
        Ok(Self {
            spec,
            output: get("--output")?.into(),
        })
    }
}
fn require_g0(class: &str) -> Result<(), String> {
    if class == "Exchanges::Gemini" {
        return Err("Gemini recorder requires E3 feeEstimate bootstrap".into());
    }
    if !w::G0_ADAPTERS.contains(&class) {
        return Err("withdrawal adapter has no G0 evidence; recording refused".into());
    }
    Ok(())
}
fn take_lock(paths: &store::Paths) -> Result<lease::EngineLock, String> {
    lease::lock(paths, chrono::Utc::now())
        .map_err(|e| format!("recorder requires the exclusive engine lock: {e:?}"))
}
fn sensitive_field(key: &str) -> bool {
    let k: String = key
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    [
        "signature",
        "secret",
        "password",
        "passphrase",
        "authorization",
        "apikey",
        "token",
        "payload",
        "query",
        "headers",
    ]
    .iter()
    .any(|s| k.contains(s))
}
fn scrub(value: Value, secrets: &[String]) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(k, v)| {
                    let value = if sensitive_field(&k) {
                        json!("[redacted]")
                    } else {
                        scrub(v, secrets)
                    };
                    // Object keys can themselves echo a credential or signed URL.
                    let key = scrub(Value::String(k), secrets)
                        .as_str()
                        .unwrap_or("[redacted]")
                        .to_owned();
                    (key, value)
                })
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.into_iter().map(|v| scrub(v, secrets)).collect())
        }
        Value::String(mut text) => {
            for secret in secrets.iter().filter(|s| !s.is_empty()) {
                text = text.replace(secret, "[redacted]");
            }
            // Free text is not a safe container for opaque signatures or credentials.
            let lower = text.to_ascii_lowercase();
            if [
                "signature",
                "secret",
                "password",
                "authorization",
                "api-key",
                "api_key",
                "token",
                "payload",
            ]
            .iter()
            .any(|k| lower.contains(k))
            {
                return json!("[redacted]");
            }
            if text.contains("http://")
                || text.contains("https://")
                || text.contains('?')
                || text.contains('=')
                || text.trim_start().starts_with('<')
            {
                text = deltabadger::engine::rules::safe_message(&text);
                if text.contains('?') {
                    text = "[redacted request]".into();
                }
            }
            json!(text)
        }
        other => other,
    }
}
#[derive(Clone)]
struct Journal(Rc<RefCell<File>>);
impl Journal {
    fn create(path: &Path) -> Result<Self, String> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(path)
            .map(|f| Self(Rc::new(RefCell::new(f))))
            .map_err(|_| "cannot create a new fixture file".into())
    }
    fn append(&self, mut row: Value) -> Result<(), String> {
        row["format"] = json!("deltabadger.withdrawal.g-live");
        row["version"] = json!(1);
        row["at"] = json!(chrono::Utc::now().to_rfc3339());
        let mut file = self.0.borrow_mut();
        writeln!(file, "{row}")
            .and_then(|()| file.sync_all())
            .map_err(|_| "fixture write failed".into())
    }
}
#[derive(Clone)]
struct Recording<T> {
    inner: T,
    journal: Journal,
    secrets: Vec<String>,
    sequence: Rc<RefCell<u64>>,
}
impl<T: Transport> Transport for Recording<T> {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.inner.now()
    }
    fn producer(&self) -> Option<model::CredentialVersion> {
        self.inner.producer()
    }
    fn redact_diagnostic(&self, text: &str) -> String {
        self.inner.redact_diagnostic(text)
    }
    async fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.send_limited(request, venue::http::MAX_BODY).await
    }
    async fn send_limited(
        &self,
        request: &HttpRequest,
        limit: usize,
    ) -> Result<HttpResponse, TransportError> {
        let sequence = {
            let mut s = self.sequence.borrow_mut();
            *s += 1;
            *s
        };
        self.journal
            .append(
                json!({"type":"request", "sequence":sequence, "method":request.method,
            "path":scrub(json!(request.path), &self.secrets)}),
            )
            .map_err(TransportError::NotSent)?;
        let result = self.inner.send_limited(request, limit).await;
        let mut secrets = self.secrets.clone();
        for (key, value) in &request.query {
            if sensitive_field(key) {
                secrets.push(value.clone());
            }
        }
        let row = match &result {
            Ok(response) => {
                let body = match serde_json::from_str::<Value>(&response.body) {
                    Ok(value) => scrub(value, &secrets),
                    Err(_) => {
                        let safe = scrub(json!(response.body), &secrets);
                        json!(deltabadger::engine::rules::safe_message(
                            safe.as_str().unwrap_or("")
                        ))
                    }
                };
                json!({"type":"answer", "sequence":sequence,"status":response.status,"body":scrub(body, &secrets)})
            }
            Err(error) => {
                let (kind, message) = match error {
                    TransportError::NotSent(s) => ("not_sent", s),
                    TransportError::MaybeSent(s) => ("maybe_sent", s),
                    TransportError::Permanent(s) => ("permanent", s),
                };
                json!({"type":"answer","sequence":sequence,"transport_error":kind,"message":scrub(json!(message), &secrets)})
            }
        };
        self.journal
            .append(row)
            .map_err(TransportError::MaybeSent)?;
        result
    }
}
async fn record<V: Venue>(
    lock: &lease::EngineLock,
    c: &rusqlite::Connection,
    cipher: &Cipher,
    adapter: w::VenueAdapter<V>,
    options: &Options,
    journal: &Journal,
) -> Result<(), String> {
    let owner = format!("recorder:{}", options.spec.scenario);
    let result = match w::begin_recorder(lock, c, cipher, &adapter, &options.spec, &SystemClock)
        .await
    {
        Ok(intent) => {
            let (account_ref, destination_digest): (String, String) = c.query_row("SELECT account_ref,destination_digest FROM rust_recorder_runs WHERE scenario_id=?1", [&options.spec.scenario], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|_| "cannot read recorder binding")?;
            journal.append(json!({"account_ref":account_ref,"destination_digest":destination_digest,"type":"intent","scenario":options.spec.scenario,"client_id":intent.client_id,
                "amount":intent.request.amount,"fee_cap":intent.request.fee_cap,"free_balance":intent.request.free_balance}))?;
            w::dispatch(c, cipher, &adapter, &intent, &SystemClock).await
        }
        Err(error) => {
            if matches!(&error, deltabadger::engine::EngineError::Data(s) if s.starts_with("scenario is already consumed"))
            {
                // Binding was checked first. Recovery never sends, even after a prepared crash.
                w::recover(c, cipher, &adapter, &owner, &SystemClock)
                    .await
                    .map_err(|_| "recorder recovery failed; use operator resolution")?;
            }
            Err(error)
        }
    };
    let state: Option<String> = c
        .query_row(
            "SELECT outcome FROM rust_recorder_runs WHERE scenario_id=?1",
            [&options.spec.scenario],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    let reason = result.as_ref().err().map(|e| match e {
        deltabadger::engine::EngineError::Data(s) => s.clone(),
        _ => "recorder refused; inspect the durable intent".into(),
    });
    journal.append(
        json!({"type":"run","scenario":options.spec.scenario,"outcome":state,"error":reason}),
    )?;
    if let Some(reason) = reason {
        return Err(format!(
            "{reason}; outcome={}",
            state.as_deref().unwrap_or("unconsumed")
        ));
    }
    if state.as_deref() == Some("unresolved") {
        return Err(
            "withdrawal unresolved; use resolve-withdrawal --recorder with venue evidence".into(),
        );
    }
    Ok(())
}
async fn run() -> Result<(), String> {
    let options = Options::parse(&std::env::args().skip(1).collect::<Vec<_>>())?;
    let env = |k: &str| std::env::var(k).ok();
    for key in [
        "DATABASE_URL",
        "PRIMARY_DATABASE_URL",
        "QUEUE_DATABASE_URL",
        "CACHE_DATABASE_URL",
        "CABLE_DATABASE_URL",
    ] {
        if env(key).is_some_and(|v| !v.is_empty()) {
            return Err("database URL overrides are not supported".into());
        }
    }
    let secret = env("SECRET_KEY_BASE")
        .filter(|s| !s.trim().is_empty())
        .ok_or("SECRET_KEY_BASE must be the install's own")?;
    let keys = EncryptionKeys::resolve(&env, &secret)
        .map_err(|_| "cannot resolve install encryption keys")?;
    let cipher = Cipher::new(&keys);
    let paths = store::Paths::from_env(
        &env,
        Path::new(&env("STORAGE_DIR").unwrap_or_else(|| "storage".into())),
    );
    let lock = take_lock(&paths)?;
    store::migrate(&paths, &lock).map_err(|_| "install schema refused")?;
    let opened = store::open(&paths).map_err(|_| "cannot open install")?;
    let class: String = opened
        .primary
        .query_row(
            "SELECT type FROM exchanges WHERE id=?1",
            [options.spec.exchange_id],
            |r| r.get(0),
        )
        .map_err(|_| "unknown exchange")?;
    require_g0(&class)?;
    let row = venue::registry::row(&class).ok_or("unknown exchange")?;
    let (mut credentials, version) = model::withdrawal_credentials(
        &opened.primary,
        &cipher,
        options.spec.user_id,
        options.spec.exchange_id,
    )
    .map_err(|_| "cannot read withdrawal credentials")?
    .ok_or("no valid withdrawal API key")?;
    let journal = Journal::create(&options.output)?;
    journal.append(json!({"type":"header","scenario":options.spec.scenario,"exchange":class,
        "user_id":options.spec.user_id,"exchange_id":options.spec.exchange_id,"asset_id":options.spec.asset_id,
        "max_amount":options.spec.max_amount,"ceiling":w::RECORDER_CEILING}))?;
    let (client, proxy) = venue::live::egress(
        &opened.primary,
        &cipher,
        &env,
        row.name_id,
        &venue::http::client(),
    )
    .map_err(|_| "cannot configure withdrawal connection")?;
    credentials.redaction_values.extend(proxy);
    let transport = match class.as_str() {
        "Exchanges::Binance" => {
            ReqwestTransport::binance(client, credentials.key.clone(), credentials.secret.clone())
        }
        "Exchanges::Mexc" => {
            ReqwestTransport::mexc(client, credentials.key.clone(), credentials.secret.clone())
        }
        "Exchanges::Kraken" => {
            ReqwestTransport::new(client, credentials.key.clone(), credentials.secret.clone())
                .kraken()
        }
        _ => return Err("unsupported recorder adapter".into()),
    }
    .with_sensitive_values(credentials.redaction_values.clone());
    let mut secrets = credentials.redaction_values.clone();
    secrets.extend([credentials.key.clone(), credentials.secret.clone()]);
    let transport = Recording {
        inner: transport,
        journal: journal.clone(),
        secrets,
        sequence: Rc::new(RefCell::new(0)),
    };
    macro_rules! execute {
        ($venue:expr) => {
            record(
                &lock,
                &opened.primary,
                &cipher,
                w::VenueAdapter::new($venue, version, row.fee_on_top),
                &options,
                &journal,
            )
            .await
        };
    }
    match class.as_str() {
        "Exchanges::Binance" => {
            execute!(
                venue::binance::BinanceVenue::new(transport, row.base, &credentials.secret)
                    .with_identity(&credentials)
            )
        }
        "Exchanges::Mexc" => {
            execute!(
                venue::mexc::MexcVenue::new(transport, row.base, &credentials.secret)
                    .with_identity(&credentials)
            )
        }
        "Exchanges::Kraken" => {
            execute!(venue::kraken::KrakenVenue::new(transport).with_identity(&credentials))
        }
        _ => Err("unsupported recorder adapter".into()),
    }
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_explicit_scenario_ceiling_and_destination() {
        assert!(Options::parse(&[]).is_err());
        let args = [
            "--scenario",
            "live-1",
            "--max-amount",
            "0.001",
            "--destination",
            r#"{"address":"listed","network":"BTC"}"#,
            "--user",
            "1",
            "--exchange",
            "1",
            "--asset",
            "1",
            "--output",
            "fixture.jsonl",
        ];
        assert!(Options::parse(&args.map(str::to_owned)).is_ok());
        for flag in ["--scenario", "--max-amount", "--destination", "--output"] {
            let mut missing = args.map(str::to_owned).to_vec();
            let i = missing.iter().position(|s| s == flag).unwrap();
            missing.drain(i..i + 2);
            assert!(Options::parse(&missing).is_err());
        }
    }
    #[test]
    fn adapter_requires_g0_and_gemini_requires_e3() {
        assert!(require_g0("Exchanges::Unknown").is_err());
        assert!(require_g0("Exchanges::Gemini").unwrap_err().contains("E3"));
        for row in deltabadger::venue::registry::ROWS.iter() {
            if !w::G0_ADAPTERS.contains(&row.exchange_type) {
                assert!(require_g0(row.exchange_type).is_err());
            }
        }
    }
    #[test]
    fn engine_lock_held_refuses_recorder() {
        let dir = tempfile::tempdir().unwrap();
        let paths = store::Paths::from_env(&|_| None, dir.path());
        let _engine = lease::lock(&paths, chrono::Utc::now()).unwrap();
        assert!(take_lock(&paths).is_err());
    }
    #[test]
    fn scrubs_nested_credentials_queries_and_non_json_bodies() {
        let raw = json!({"https://venue/path?signature=sig&amount=1":"echo", "signature":"sig", "nested":[{"secret":"secret-value", "message":"https://venue/path?signature=sig&amount=1", "echo":"api-key-value"}], "amount":"0.001", "id":"venue-id"});
        let scrubbed = scrub(raw, &["api-key-value".into(), "secret-value".into()]);
        let text = scrubbed.to_string();
        assert!(!text.contains("secret-value"));
        assert!(!text.contains("api-key-value"));
        assert!(!text.contains("sig\""));
        assert!(!text.contains("?signature="));
        assert_eq!(scrubbed["id"], "venue-id");
        assert_eq!(scrubbed["amount"], "0.001");
        assert!(!scrub(json!("signature=abc secret: xyz"), &[])
            .to_string()
            .contains("xyz"));
    }
    #[tokio::test]
    async fn scripted_answers_are_durable_and_scrubbed_without_changing_adapter_answers() {
        use venue::http::ScriptedTransport;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.jsonl");
        let journal = Journal::create(&path).unwrap();
        assert!(Journal::create(&path).is_err());
        let inner = ScriptedTransport::default();
        inner.reply("GET /answer", 200, json!({"id":"withdrawal-1", "amount":"0.0009", "signature":"wire-signature", "secret":"private-value"}));
        inner.network("GET /lost", "post_send", "reply lost");
        let recorder = Recording {
            inner,
            journal,
            secrets: vec!["private-value".into()],
            sequence: Rc::new(RefCell::new(0)),
        };
        let mut request = HttpRequest {
            method: "GET",
            base: "https://venue".into(),
            path: "/answer".into(),
            query: vec![],
            body: None,
            not_after: None,
        };
        let answer = recorder.send(&request).await.unwrap();
        assert!(
            answer.body.contains("wire-signature"),
            "adapter sees the original answer"
        );
        request.path = "/lost".into();
        assert!(recorder.send(&request).await.is_err());
        let text = std::fs::read_to_string(path).unwrap();
        assert!(!text.contains("private-value"));
        assert!(!text.contains("wire-signature"));
        let rows: Vec<Value> = text
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1]["status"], 200);
        assert_eq!(rows[1]["body"]["amount"], "0.0009");
        assert!(rows[3]["transport_error"].is_string());
    }
    #[tokio::test]
    async fn journal_failure_before_send_refuses_and_after_reply_is_ambiguous() {
        #[derive(Clone)]
        struct FailAfterReply {
            inner: venue::http::ScriptedTransport,
            journal: Journal,
            path: PathBuf,
        }
        impl Transport for FailAfterReply {
            async fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
                let response = self.inner.send(request).await;
                *self.journal.0.borrow_mut() = File::open(&self.path).unwrap();
                response
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture");
        let journal = Journal::create(&path).unwrap();
        let inner = venue::http::ScriptedTransport::default();
        inner.reply("POST /withdraw", 200, json!({"id":"accepted"}));
        let request = HttpRequest {
            method: "POST",
            base: "https://venue".into(),
            path: "/withdraw".into(),
            query: vec![],
            body: None,
            not_after: None,
        };
        let recorder = Recording {
            inner: FailAfterReply {
                inner: inner.clone(),
                journal: journal.clone(),
                path,
            },
            journal,
            secrets: vec![],
            sequence: Rc::new(RefCell::new(0)),
        };
        assert!(matches!(
            recorder.send(&request).await,
            Err(TransportError::MaybeSent(_))
        ));
        assert_eq!(inner.requests().len(), 1);
        assert!(matches!(
            recorder.send(&request).await,
            Err(TransportError::NotSent(_))
        ));
        assert_eq!(
            inner.requests().len(),
            1,
            "cannot send when the pre-request journal write fails"
        );
    }
}
