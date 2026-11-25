//! PLAN-710-mcpadapter (D-IMPORT-API): `import_transactions_csv`, the tracker's file import over MCP and REST
//! (`POST /api/v1/tracker/import`). An adapter, no import logic: a request is one of three closed forms, each one call of
//! the shared operation (`tracker::import::service`), whose `Outcome` it serialises with the web's own texts in the owner's
//! language. The bearer is checked before any body is read, after every wait, while a file arrives, once it has arrived,
//! and inside the operation's transaction.
use super::bearer::{self, Refusal};
use super::rest::{self, Answer};
use super::{auth, consent, i18n, locale, multipart, App, Params, WebError, BODY_READ_TIMEOUT, FORM_LIMIT, IMPORT_LIMIT, IMPORT_READ_TIMEOUT, IMPORT_SLOT_WAIT};
use crate::tracker::import::run::{self, Accept, Answer as Asked, Kind};
use crate::tracker::import::service::{self, Denied, Outcome};
use axum::body::Body;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Duration;

pub const TOOL: &str = "import_transactions_csv";
/// PLAN-710-mcpadapter I2: the API's markers, which no web import request may carry.
pub const MARKERS: [&str; 2] = ["upload", "choice"];
/// H2: what a REST query may never carry on this route (every form's marker, the web's included).
const QUERY_MARKERS: [&str; 6] = ["upload", "choice", "cancel", "token", "handle", "api_key_id"];
/// J1: while a file arrives, its bearer is checked again at least this often, by bytes and by time.
const CHECK_BYTES: usize = 1024 * 1024;
const CHECK_EVERY: Duration = Duration::from_secs(5);
pub const RETRY_AFTER: u64 = 5;
/// Test hook: how many bytes of the body a read had kept when a check stopped it.
#[doc(hidden)]
pub static COLLECTED_AT_DENIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// H8: every (status, code) a REST import answer carries, each on its row of manual/api.md's status table.
pub const CODES: [(u16, &str); 17] = [
    (400, "ambiguous_request"), (400, "malformed"), (401, "missing_token"), (401, "invalid_token"), (401, "token_revoked"), (401, "token_expired"),
    (401, "user_not_found"), (403, "insufficient_scope"), (403, "tool_disabled"), (403, "account_inactive"), (422, "no_file"), (422, "malformed"),
    (422, "unreadable"), (422, "unknown_timezone"), (422, "no_account_for_format"), (422, "refused"), (503, "busy"),
];

/// F1: the REST route, with any `.format` suffix (as `rest::route` reads it).
pub fn rest_path(route_path: &str) -> bool {
    route_path.strip_prefix("/api/v1/tracker/import").is_some_and(|rest| rest.is_empty() || rest.strip_prefix('.').is_some_and(|f| !f.is_empty() && !f.contains(['.', '/'])))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface { Rest, Mcp }
impl Surface {
    fn scope(self) -> &'static str { match self { Surface::Rest => "api", Surface::Mcp => "mcp" } }
}

/// Who may import, read only (the resolver opens no transaction of its own): the bearer as the resolver judges it, the
/// desktop's owner, the tool on and granted to the client, the owner active (not locked, confirmed), and then (H1/I1) the
/// bearer judged again by a fresh clock, after every lookup.
pub fn authorized(c: &Connection, app: &App, surface: Surface, authorization: Option<&str>) -> Result<Result<i64, Denied>, WebError> {
    let resolve = || -> Result<Result<bearer::Bearer, Refusal>, WebError> {
        Ok(bearer::resolve(c, authorization, surface.scope(), app.clock.as_ref())?.and_then(|who| if app.desktop_owner_allowed(who.user_id) { Ok(who) } else { Err(Refusal::UserNotFound) }))
    };
    let who = match resolve()? { Ok(who) => who, Err(why) => return Ok(Err(Denied::Bearer(why))) };
    let (enabled, granted) = match surface { Surface::Rest => consent::rest_access(c, who.user_id, who.application_id)?, Surface::Mcp => consent::mcp_access(c, who.user_id, who.application_id)? };
    if !(enabled.iter().any(|t| t == TOOL) && granted.iter().any(|t| t == TOOL)) { return Ok(Err(Denied::Ungranted)); }
    let now = app.now();
    if !auth::User::find(c, who.user_id)?.is_some_and(|user| user.confirmed && !user.locked(now)) { return Ok(Err(Denied::Inactive)); }
    match resolve()? { Ok(again) if again == who => {}, Ok(_) => return Ok(Err(Denied::Bearer(Refusal::Invalid))), Err(why) => return Ok(Err(Denied::Bearer(why))) }
    // Branch review r1 #2: last of all, after every lookup (the resolver's own of the owner included), the token's row
    // judged by a fresh clock.
    let Some(token) = bearer::bearer_token(authorization).map(|t| super::oauth::AccessToken::find(c, "token", t)).transpose()?.flatten() else { return Ok(Err(Denied::Bearer(Refusal::Invalid))) };
    let now = app.now();
    Ok(if token.id != who.token_id { Err(Denied::Bearer(Refusal::Invalid)) } else if token.revoked(now) { Err(Denied::Bearer(Refusal::Revoked)) } else if token.expired(now) { Err(Denied::Bearer(Refusal::Expired)) } else { Ok(who.user_id) })
}

/// The check the operation runs inside its transaction.
fn still(app: &App, surface: Surface, authorization: Option<String>) -> service::SignedIn {
    let app = app.clone();
    Arc::new(move |c: &Connection| match authorized(c, &app, surface, authorization.as_deref()) {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(why)) => Err(why),
        Err(e) => Err(Denied::Db(format!("{e:?}"))),
    })
}

/// I5/H6: a denial as REST answers it (MCP returns the same code).
pub fn refusal(why: &Denied) -> (StatusCode, &'static str, String) {
    match why {
        Denied::Bearer(r) => (if *r == Refusal::InsufficientScope { StatusCode::FORBIDDEN } else { StatusCode::UNAUTHORIZED }, r.code(), r.message().to_string()),
        Denied::Ungranted => (StatusCode::FORBIDDEN, "tool_disabled", format!("Tool '{TOOL}' is disabled for this user.")),
        Denied::Inactive => (StatusCode::FORBIDDEN, "account_inactive", "This account is locked or not confirmed.".into()),
        Denied::SignedOut => (StatusCode::UNAUTHORIZED, Refusal::Invalid.code(), Refusal::Invalid.message().into()),
        Denied::Db(_) => (StatusCode::INTERNAL_SERVER_ERROR, "error", "An unexpected error occurred.".into()),
    }
}

/// One request's form (H3), each of exactly its members.
#[derive(Debug, PartialEq)]
pub enum Form { Fresh { format: &'static str, bytes: Vec<u8>, filename: Option<String> }, Confirm { upload: String, choice: String }, Cancel { upload: String } }

impl Form {
    fn name(&self) -> &'static str { match self { Form::Fresh { .. } => "fresh", Form::Confirm { .. } => "confirm", Form::Cancel { .. } => "cancel" } }
}

/// A JSON request's form (MCP's arguments; REST's body, where `fresh` is not one): exactly one form's members with their
/// types, else `ambiguous_request`; a fresh upload's content that is no base64 is `malformed`.
pub fn form_of(args: &Map<String, Value>, fresh: bool) -> Result<Form, &'static str> {
    let only = |names: &[&str]| args.keys().all(|k| names.contains(&k.as_str()));
    let text = |name: &str| args.get(name).and_then(Value::as_str).map(str::to_string);
    if args.contains_key("content") {
        if !fresh || !only(&["format", "content", "filename"]) { return Err("ambiguous_request"); }
        let named = match args.get("format") { None => None, Some(v) => Some(run::FORMATS.iter().copied().find(|f| v.as_str() == Some(*f)).ok_or("ambiguous_request")?) };
        let filename = match args.get("filename") { None => None, Some(Value::String(f)) => Some(f.clone()), Some(_) => return Err("ambiguous_request") };
        let content = text("content").ok_or("ambiguous_request")?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(content.trim()).map_err(|_| "malformed")?;
        // D-IMPORT-DETECT: a format left out is the one the file's header names, as the web's import reads it.
        let format = match named { Some(format) => format, None => run::detect(&bytes).ok_or("unreadable")? };
        return Ok(Form::Fresh { format, bytes, filename });
    }
    if args.contains_key("cancel") {
        if !only(&["upload", "cancel"]) || args.get("cancel") != Some(&Value::Bool(true)) { return Err("ambiguous_request"); }
        return text("upload").map(|upload| Form::Cancel { upload }).ok_or("ambiguous_request");
    }
    if !only(&["upload", "choice"]) { return Err("ambiguous_request"); }
    match (text("upload"), text("choice")) { (Some(upload), Some(choice)) => Ok(Form::Confirm { upload, choice }), _ => Err("ambiguous_request") }
}

/// I3: whether some object of the JSON text `raw` names a member twice, at any depth (a text that is no JSON is not
/// checked here: its parser answers it).
pub fn duplicate_members(raw: &[u8]) -> bool {
    use serde::de::{DeserializeSeed, Deserializer, Error, MapAccess, SeqAccess, Visitor};
    struct Walk;
    impl<'de> DeserializeSeed<'de> for Walk {
        type Value = ();
        fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> { d.deserialize_any(Walk) }
    }
    impl<'de> Visitor<'de> for Walk {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str("JSON") }
        fn visit_bool<E>(self, _: bool) -> Result<(), E> { Ok(()) }
        fn visit_i64<E>(self, _: i64) -> Result<(), E> { Ok(()) }
        fn visit_u64<E>(self, _: u64) -> Result<(), E> { Ok(()) }
        fn visit_f64<E>(self, _: f64) -> Result<(), E> { Ok(()) }
        fn visit_str<E>(self, _: &str) -> Result<(), E> { Ok(()) }
        fn visit_unit<E>(self) -> Result<(), E> { Ok(()) }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> { while seq.next_element_seed(Walk)?.is_some() {} Ok(()) }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            let mut seen = std::collections::HashSet::new();
            while let Some(key) = map.next_key::<String>()? {
                if !seen.insert(key) { return Err(A::Error::custom("a member named twice")); }
                map.next_value_seed(Walk)?;
            }
            Ok(())
        }
    }
    let mut d = serde_json::Deserializer::from_slice(raw);
    let walked = Walk.deserialize(&mut d);
    walked.is_err() && serde_json::from_slice::<serde::de::IgnoredAny>(raw).is_ok()
}

/// The owner's language, as the web's pages pick it.
fn language(c: &Connection, user: i64) -> Result<&'static str, WebError> {
    Ok(locale::switch(None, auth::User::find(c, user)?.and_then(|u| u.locale).as_deref()))
}

fn destination(d: &run::Destination) -> Value {
    json!({"exchange_id": d.exchange_id, "account_id": d.account_id, "rows": d.rows,
           "prospective": d.prospective.map(|p| json!({"key_id": p.key_id, "number": p.number}))})
}

/// An `Outcome` as data, or the failure it is (status, code, message), in the owner's language `l` (rule 2).
pub fn outcome(l: &str, outcome: Outcome) -> Result<Value, (StatusCode, &'static str, String)> {
    let lines = |w: &run::Warning| crate::web::tracker::import::plain_lines(l, w);
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    match outcome {
        Outcome::Written { imported, duplicates, skipped, unrecognised, unrouted, generic, destinations } => Ok(json!({
            "imported": imported, "duplicates": duplicates, "skipped": skipped, "generic": generic, "unrouted": unrouted, "unrecognised": unrecognised,
            "destinations_written": destinations.iter().map(|(exchange_id, account_id, rows)| json!({"exchange_id": exchange_id, "account_id": account_id, "rows": rows})).collect::<Vec<_>>(),
        })),
        Outcome::Cancelled => Ok(json!({"cancelled": true})),
        Outcome::NoFile => Err((unprocessable, "no_file", i18n::text(l, "tracker.import.error_no_file", &[]))),
        Outcome::Error(error) => Err((unprocessable, error, i18n::text(l, &format!("tracker.import.error_{error}"), &[]))),
        Outcome::Refused(reason) => {
            crate::engine::log(&format!("[Binance] import refused: {reason}"));
            Err((unprocessable, "refused", i18n::text(l, crate::web::tracker::import::refused_key(&reason), &[])))
        }
        Outcome::Denied(why) => { let (status, code, message) = refusal(&why); Err((status, code, message)) }
        Outcome::Question { token, format, answer, kinds, reaches, destinations, warning_accounts } => {
            let (warnings, choices, venue) = match &answer {
                Asked::Warned(warnings, choices) => (warnings.as_slice(), choices.as_slice(), false),
                Asked::Ambiguous(choices) => (&[][..], choices.as_slice(), !choices.iter().any(|k| k.new)),
                _ => return Err((unprocessable, "no_file", i18n::text(l, "tracker.import.error_no_file", &[]))),
            };
            // The page's own texts: a Deltabadger CSV's one answer, a venue's selector, or one named button per destination.
            let csv = matches!(choices, [only] if matches!(only.accept, Some(Accept::Anyway | Accept::Rest | Accept::Import)));
            let (kind, shown): (&str, Vec<String>) = match &answer {
                Asked::Warned(..) if csv => ("warning", warnings.iter().flat_map(&lines).collect()),
                Asked::Warned(..) => ("warning", warnings.iter().filter(|w| matches!(w, run::Warning::Changed | run::Warning::Retained | run::Warning::Dust | run::Warning::Undone(..))).flat_map(&lines).collect()),
                _ if venue => ("venue", vec![i18n::text(l, "tracker.import.account", &[])]),
                _ => ("account", vec![i18n::text(l, "tracker.import.which_of_accounts", &[])]),
            };
            let buttons = crate::web::tracker::import::plain_buttons(l, choices);
            let listed: Vec<Value> = choices.iter().enumerate().map(|(n, k)| {
                let at = kinds.get(n).copied().unwrap_or(Kind::Accept);
                let what = match (at, k.accept) {
                    (Kind::Select, _) => "select", (Kind::Authorize, _) => "authorize",
                    (_, Some(Accept::Anyway)) => "anyway", (_, Some(Accept::Rest)) => "rest", (_, Some(Accept::Import)) => "import",
                    (_, Some(Accept::New)) => "new", (_, None) if k.new => "new", _ => "account",
                };
                let label = if csv { i18n::text(l, match k.accept { Some(Accept::Rest) => "tracker.import.import_rest", Some(Accept::Anyway) => "tracker.import.import_anyway", _ => "tracker.import.confirm" }, &[]) }
                    else if venue { k.shown.plain() } else { buttons[n].clone() };
                let counted = !(venue || csv);
                json!({"choice": k.value, "kind": what, "label": label, "rows_in_file": counted.then_some(k.count), "exchange_id": k.venue, "account_id": k.account,
                       "new_account": what == "new", "notes": k.notes.iter().flat_map(&lines).collect::<Vec<_>>(),
                       "destinations": reaches.get(n).into_iter().flatten().map(destination).collect::<Vec<_>>()})
            }).collect();
            Ok(json!({"upload": token, "format": format, "kind": kind, "actions": if venue { json!(["choose"]) } else { json!(["choose", "cancel"]) }, "lines": shown, "choices": listed,
                      "destinations": destinations.iter().map(destination).collect::<Vec<_>>(),
                      "warning_accounts": warning_accounts.iter().map(|a| json!({"account_id": a.account_id, "exchange_id": a.exchange_id, "label": a.label.plain(), "rows": a.rows})).collect::<Vec<_>>()}))
        }
    }
}

/// What a request is answered (before the surfaces render it).
pub enum Done { Data(Value), Failed(StatusCode, &'static str, String), Busy }

impl Done {
    /// Rule 4: what history keeps of it: its outcome and counts, no text.
    pub fn summary(&self) -> Value {
        match self {
            Done::Data(v) if v.get("upload").is_some() => json!({"outcome": "question", "kind": v["kind"], "choices": v["choices"].as_array().map_or(0, Vec::len)}),
            Done::Data(v) if v.get("cancelled").is_some() => json!({"outcome": "cancelled"}),
            Done::Data(v) => json!({"outcome": "written", "counts": {"imported": v["imported"], "duplicates": v["duplicates"], "skipped": v["skipped"]}}),
            Done::Failed(_, code, _) => json!({"outcome": code}),
            Done::Busy => json!({"outcome": "busy"}),
        }
    }
}

/// F3: a slot within IMPORT_SLOT_WAIT, and whether it was waited for; None: busy.
async fn slot(app: &App) -> (Option<service::Permit>, bool) {
    match app.import_slots.clone().try_acquire_owned() {
        Ok(permit) => (Some(permit), false),
        Err(_) => (tokio::time::timeout(IMPORT_SLOT_WAIT, app.import_slots.clone().acquire_owned()).await.ok().and_then(Result::ok), true),
    }
}

/// `authorized` off the request's own thread, its denial as a failure.
async fn check(app: &App, surface: Surface, authorization: Option<String>) -> Result<i64, Done> {
    let inner = app.clone();
    match app.db(move |c| authorized(c, &inner, surface, authorization.as_deref())).await {
        Ok(Ok(user)) => Ok(user),
        Ok(Err(why)) => { let (status, code, message) = refusal(&why); Err(Done::Failed(status, code, message)) }
        Err(_) => Err(Done::Failed(StatusCode::INTERNAL_SERVER_ERROR, "error", "An unexpected error occurred.".into())),
    }
}

/// The one operation a form is, holding `permit` (F3: the worker owns it), and its outcome in the owner's language.
async fn perform(app: &App, surface: Surface, authorization: Option<String>, user: i64, form: Form, permit: service::Permit) -> Done {
    let signed_in = still(app, surface, authorization);
    let answered = match form {
        Form::Fresh { bytes, .. } if crate::web::tracker::import::blank(&bytes) => Ok(Outcome::NoFile),
        Form::Fresh { format, bytes, filename } => service::upload(app, user, format, bytes, filename, signed_in, permit).await,
        Form::Confirm { upload, choice } if service::token_shaped(&upload) && service::handle_shaped(&choice) => service::confirm(app, user, upload, choice, signed_in, permit).await,
        Form::Cancel { upload } if service::token_shaped(&upload) => service::cancel(app, user, upload, signed_in, permit).await,
        Form::Confirm { .. } | Form::Cancel { .. } => Ok(Outcome::NoFile),
    };
    let outcome = match answered { Ok(outcome) => outcome, Err(_) => return Done::Failed(StatusCode::INTERNAL_SERVER_ERROR, "error", "An unexpected error occurred.".into()) };
    let l = match app.db(move |c| language(c, user)).await { Ok(l) => l, Err(_) => locale::switch(None, None) };
    match self::outcome(l, outcome) { Ok(data) => Done::Data(data), Err((status, code, message)) => Done::Failed(status, code, message) }
}

/// A form already read (MCP; REST's JSON): the slot (F3), the check after waiting for it, the operation.
async fn admitted(app: &App, surface: Surface, authorization: Option<String>, user: i64, form: Form) -> Done {
    let (permit, waited) = slot(app).await;
    let Some(permit) = permit else { return match check(app, surface, authorization).await { Ok(_) => Done::Busy, Err(denied) => denied } };
    if waited { if let Err(denied) = check(app, surface, authorization.clone()).await { return denied; } }
    perform(app, surface, authorization, user, form, permit).await
}

// ---------------------------------------------------------------- MCP

/// An MCP call past its gate and its schema: run after the dispatch has let `App::db` go.
pub struct Job { pub user: i64, pub authorization: Option<String>, pub form: Form }

/// What MCP's history keeps of a call's request (rule 4): its form, the file's size and digest, never its content or name.
pub fn request_summary(args: &Value) -> Value {
    let form = args.as_object().map(|a| if a.contains_key("content") { "fresh" } else if a.contains_key("cancel") { "cancel" } else { "confirm" }).unwrap_or("invalid");
    let content = args.get("content").and_then(Value::as_str).and_then(|c| { use base64::Engine; base64::engine::general_purpose::STANDARD.decode(c.trim()).ok() });
    let digest = content.as_ref().map(|b| { use sha2::Digest; hex::encode(sha2::Sha256::digest(b)) });
    json!({"tool": TOOL, "form": form, "bytes": content.as_ref().map(Vec::len), "sha256": digest})
}

/// The call's arguments checked (H3/G2), inside the dispatch: its job, or its answer now.
pub fn mcp_call(c: &Connection, app: &App, authorization: Option<&str>, args: &Value) -> Result<Result<Job, Value>, WebError> {
    let user = match authorized(c, app, Surface::Mcp, authorization)? { Ok(user) => user, Err(why) => { let (_, code, message) = refusal(&why); return Ok(Err(failed(code, &message, None))); } };
    let schema = super::mcp::protocol::metadata()["tools"].as_array().into_iter().flatten().find(|t| t["name"] == TOOL).map(|t| &t["inputSchema"]).ok_or_else(|| WebError::Config("missing import schema".into()))?;
    let errors = super::mcp::protocol::validate(args, schema, "");
    if !errors.is_empty() { return Ok(Err(super::mcp::protocol::tool_text(&format!("Invalid input: {}", errors.join(", ")), true))); }
    Ok(match form_of(args.as_object().unwrap_or(&Map::new()), true) {
        Ok(form) => Ok(Job { user, authorization: authorization.map(str::to_string), form }),
        Err(code) => Err(failed(code, code, None)),
    })
}

fn failed(code: &str, message: &str, retry_after: Option<u64>) -> Value {
    let mut body = json!({"code": code, "message": message});
    if let Some(after) = retry_after { body["retry_after"] = json!(after); }
    super::mcp::protocol::tool_text(&rest::encode(&body), true)
}

/// An MCP job's tool result, and what history keeps of it.
pub async fn mcp_run(app: &App, job: Job) -> (Value, Value) {
    let form = job.form.name();
    let done = admitted(app, Surface::Mcp, job.authorization, job.user, job.form).await;
    let mut summary = done.summary();
    summary["tool"] = json!(TOOL);
    summary["form"] = json!(form);
    let result = match done {
        Done::Data(data) => super::mcp::protocol::tool_text(&rest::encode(&data), false),
        Done::Failed(_, code, message) => failed(code, &message, None),
        Done::Busy => failed("busy", BUSY, Some(RETRY_AFTER)),
    };
    (result, summary)
}

// ---------------------------------------------------------------- REST

fn envelope(app: &App, done: Done) -> Response {
    let busy = matches!(done, Done::Busy);
    let answer = match done {
        Done::Data(data) => Answer::Json(StatusCode::OK, rest::encode(&json!({"data": data, "error": null}))),
        // An unexpected error is REST's 500, below the controllers (no envelope).
        Done::Failed(StatusCode::INTERNAL_SERVER_ERROR, _, message) => Answer::Below(Box::new(WebError::Config(message).into_response())),
        Done::Failed(status, code, message) => rest::failure(status, code, &message),
        Done::Busy => Answer::Json(StatusCode::SERVICE_UNAVAILABLE, rest::encode(&json!({"data": null, "error": {"code": "busy", "message": BUSY, "retry_after": RETRY_AFTER}}))),
    };
    let mut response = rest::finish(app, answer, false, None);
    if busy { response.headers_mut().insert(header::RETRY_AFTER, header::HeaderValue::from_static("5")); }
    response
}

const BUSY: &str = "Every import slot is taken; try again in a few seconds.";

/// How a REST body read ended other than with the whole body.
enum Cut { Denied(Done), TimedOut, TooLarge, Broken }

/// J1: the multipart body, at most IMPORT_LIMIT bytes within IMPORT_READ_TIMEOUT, the bearer checked again after every
/// CHECK_BYTES and at least every CHECK_EVERY: a denial stops the read there, what was read dropped.
async fn read_checked(app: &App, authorization: &Option<String>, body: Body) -> Result<Vec<u8>, Cut> {
    use futures_util::StreamExt;
    let started = tokio::time::Instant::now();
    let deadline = started + IMPORT_READ_TIMEOUT;
    let mut stream = body.into_data_stream();
    let (mut out, mut since, mut last) = (Vec::new(), 0usize, started);
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline { return Err(Cut::TimedOut); }
        let next = (last + CHECK_EVERY).min(deadline);
        let polled = tokio::time::timeout_at(next, stream.next()).await;
        let due = match polled {
            Err(_) => true,
            Ok(None) => return Ok(out),
            Ok(Some(Err(_))) => return Err(Cut::Broken),
            Ok(Some(Ok(chunk))) => {
                if out.len() + chunk.len() > IMPORT_LIMIT { return Err(Cut::TooLarge); }
                // Branch review r1 #8: checked before a chunk takes the read past CHECK_BYTES since the last check; a chunk
                // crossing it is kept up to it, then checked, then the rest.
                let mut rest = &chunk[..];
                while since + rest.len() >= CHECK_BYTES {
                    let (now, later) = rest.split_at(CHECK_BYTES - since);
                    out.extend_from_slice(now);
                    rest = later;
                    if let Err(denied) = check(app, Surface::Rest, authorization.clone()).await { COLLECTED_AT_DENIAL.store(out.len(), std::sync::atomic::Ordering::SeqCst); return Err(Cut::Denied(denied)); }
                    (since, last) = (0, tokio::time::Instant::now());
                }
                out.extend_from_slice(rest);
                since += rest.len();
                tokio::time::Instant::now() >= last + CHECK_EVERY
            }
        };
        if due {
            if let Err(denied) = check(app, Surface::Rest, authorization.clone()).await { COLLECTED_AT_DENIAL.store(out.len(), std::sync::atomic::Ordering::SeqCst); return Err(Cut::Denied(denied)); }
            (since, last) = (0, tokio::time::Instant::now());
        }
    }
}

/// G2/F5/J2: a fresh upload's parts: one `file` (file-valued), at most one `format` and one `filename` (text, the same
/// name as the file's own), nothing read two ways, nothing else.
fn fresh(parsed: multipart::Parsed) -> Result<Form, &'static str> {
    if parsed.ambiguous || parsed.files > 1 || !parsed.file_parts.is_empty() { return Err("ambiguous_request"); }
    let mut format = None;
    let mut named = None;
    for (name, value) in parsed.fields {
        let slot = match name.as_str() { "format" => &mut format, "filename" => &mut named, _ => return Err("ambiguous_request") };
        if slot.replace(value).is_some() { return Err("ambiguous_request"); }
    }
    let chosen = match format { None => None, Some(f) => Some(run::FORMATS.iter().copied().find(|x| *x == f).ok_or("ambiguous_request")?) };
    let Some((bytes, filename)) = parsed.file else { return Err("no_file") };
    if named.is_some_and(|n| n != filename) { return Err("ambiguous_request"); }
    // D-IMPORT-DETECT: a format left out is the one the file's header names, as the web's import reads it.
    let format = match chosen { Some(format) => format, None => run::detect(&bytes).ok_or("unreadable")? };
    Ok(Form::Fresh { format, bytes, filename: Some(filename) })
}

/// `POST /api/v1/tracker/import`, before any body is read (F1): the bearer, the owner, the grant and the account; the
/// query's markers (H2); then a fresh upload's slot before its body (G5), or a confirmation's small JSON before its slot.
pub async fn rest(app: App, headers: HeaderMap, params: Arc<Params>, body: Body) -> Response {
    let authorization = super::header_text(&headers, "authorization").map(str::to_string);
    let user = match check(&app, Surface::Rest, authorization.clone()).await { Ok(user) => user, Err(denied) => return envelope(&app, denied) };
    let ambiguous = || envelope(&app, Done::Failed(StatusCode::BAD_REQUEST, "ambiguous_request", "The request names more than one import form, or a member it does not take.".into()));
    if params.query.iter().any(|(k, _)| QUERY_MARKERS.contains(&k.as_str())) { return ambiguous(); }
    let content_type = super::header_text(&headers, "content-type").unwrap_or("").to_string();
    if let Some(boundary) = multipart::boundary(&content_type) {
        let (permit, waited) = slot(&app).await;
        let Some(permit) = permit else { return envelope(&app, match check(&app, Surface::Rest, authorization).await { Ok(_) => Done::Busy, Err(denied) => denied }) };
        if waited { if let Err(denied) = check(&app, Surface::Rest, authorization.clone()).await { return envelope(&app, denied); } }
        let bytes = match read_checked(&app, &authorization, body).await {
            Ok(bytes) => bytes,
            Err(Cut::Denied(denied)) => return envelope(&app, denied),
            Err(Cut::TimedOut) => return (StatusCode::REQUEST_TIMEOUT, [(header::CONNECTION, "close")], "The form did not arrive in time\n").into_response(),
            Err(Cut::TooLarge) => return (StatusCode::PAYLOAD_TOO_LARGE, "Form too large\n").into_response(),
            Err(Cut::Broken) => return envelope(&app, Done::Failed(StatusCode::BAD_REQUEST, "malformed", "The upload could not be read.".into())),
        };
        // I4: received whole; checked once more before anything of it is read.
        if let Err(denied) = check(&app, Surface::Rest, authorization.clone()).await { return envelope(&app, denied); }
        let form = match multipart::parse(&bytes, &boundary) {
            Ok(parsed) => fresh(parsed),
            Err(_) => return envelope(&app, Done::Failed(StatusCode::BAD_REQUEST, "malformed", "The upload could not be read.".into())),
        };
        drop(bytes);
        return match form {
            Ok(form) => envelope(&app, perform(&app, Surface::Rest, authorization, user, form, permit).await),
            Err("no_file") => { let l = app.db(move |c| language(c, user)).await.unwrap_or("en"); envelope(&app, Done::Failed(StatusCode::UNPROCESSABLE_ENTITY, "no_file", i18n::text(l, "tracker.import.error_no_file", &[]))) }
            // D-IMPORT-DETECT: no format named and none in the header: the web's answer to the same file.
            Err("unreadable") => { let l = app.db(move |c| language(c, user)).await.unwrap_or("en"); envelope(&app, Done::Failed(StatusCode::UNPROCESSABLE_ENTITY, "unreadable", i18n::text(l, "tracker.import.error_unknown_format", &[]))) }
            Err(_) => ambiguous(),
        };
    }
    let json_type = matches!(content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase().as_str(), "application/json" | "text/x-json" | "application/jsonrequest");
    if !json_type { return ambiguous(); }
    let raw = match tokio::time::timeout(BODY_READ_TIMEOUT, axum::body::to_bytes(body, FORM_LIMIT)).await {
        Err(_) => return (StatusCode::REQUEST_TIMEOUT, [(header::CONNECTION, "close")], "The form did not arrive in time\n").into_response(),
        Ok(Err(_)) => return (StatusCode::PAYLOAD_TOO_LARGE, "Form too large\n").into_response(),
        Ok(Ok(raw)) => raw,
    };
    // Branch review r1 #7: the body arrived: checked before any of it is read.
    if let Err(denied) = check(&app, Surface::Rest, authorization.clone()).await { return envelope(&app, denied); }
    if duplicate_members(&raw) { return ambiguous(); }
    let form = match serde_json::from_slice::<Value>(&raw) {
        Ok(Value::Object(args)) => form_of(&args, false),
        _ => Err("ambiguous_request"),
    };
    match form {
        Ok(form) => envelope(&app, admitted(&app, Surface::Rest, authorization, user, form).await),
        Err(_) => ambiguous(),
    }
}
