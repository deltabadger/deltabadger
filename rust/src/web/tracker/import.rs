//! Tracker::ImportsController: the dialog, and a file read into the ledger (tracker::import::run). The one question it
//! can ask (which venue or account a file came from, #710 7a §3) parks the upload at `<tmp>/imports/<user>/<token>.csv` and
//! its zone in `session["import_offsets"][token]`, the paths and keys 2.x uses, until the answer comes back.
use super::{auth, Ctx, WebError};
use crate::tracker::import::run::{self, Answer};
use crate::web::bots::{Segmented, SegmentedOption};
use crate::web::{flash, i18n, turbo};
use askama::Template;
use axum::{extract::Extension, http::{header, StatusCode}, response::{IntoResponse, Response}};
use std::path::PathBuf;

pub use run::{DEFAULT_FORMAT, FORMATS};

/// UPLOAD_TTL: a parked upload nobody answered for is swept on the next park after an hour.
const UPLOAD_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

#[derive(Template)]
#[template(path = "tracker/import_new.html")]
struct New<'a> { ctx: &'a Ctx, csrf: String, formats: String, default_format: &'static str }

/// What create.html.erb says when no rows landed.
enum Body {
    Error(String),
    Refused(String),
    Ambiguous { csrf: String, token: String, format: &'static str, accounts: String, first: String, question: &'static str },
    /// #710 7a: what the import warns about, its answers, and Cancel.
    Warned { csrf: String, token: String, format: &'static str, lines: Vec<String>, accounts: String, first: String },
    NothingNew { skipped: Option<String>, unrouted: Option<String>, unrecognised: Option<String> },
}

#[derive(Template)]
#[template(path = "tracker/import_create.html")]
struct Create<'a> { ctx: &'a Ctx, form: bool, body: Body }

fn html(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

pub async fn new(Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)); }
    let options = FORMATS.iter().map(|&value| SegmentedOption {
        value: value.into(), label: ctx.t(&format!("tracker.import.format_{value}")), active: value == DEFAULT_FORMAT, href: None,
    }).collect();
    let formats = Segmented { fluid: true, label: ctx.t("tracker.import.format"), key: None, options, links: false, frame: None }.render()?;
    Ok(html(StatusCode::OK, New { ctx: &ctx, csrf: ctx.csrf_token(), formats, default_format: DEFAULT_FORMAT }.render()?))
}

/// 2.x's `text.blank?` on the upload's binary string: empty, or ASCII whitespace only.
fn blank(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
}

/// `TOKEN = /\A[a-f0-9]{16}\z/`: the second request of the Binance/Binance.US question.
fn token(value: Option<&str>) -> bool {
    value.is_some_and(|t| t.len() == 16 && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

fn present(value: Option<&str>) -> Option<&str> { value.filter(|v| !crate::v2::blank(v)) }

fn uploads(ctx: &Ctx, user: i64) -> PathBuf { ctx.app.config.tmp_dir.join("imports").join(user.to_string()) }

/// #parked_offsets: only the offsets whose files are still parked, so a stale cookie cannot outlive the upload.
fn parked_offsets(ctx: &Ctx, dir: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    let mut offsets = ctx.session.lock().import_offsets.clone();
    offsets.retain(|token, _| dir.join(format!("{token}.csv")).is_file());
    offsets
}

/// `import_venues` of the uploads still parked (as `parked_offsets`).
fn parked_venues(ctx: &Ctx, dir: &std::path::Path) -> std::collections::BTreeMap<String, i64> {
    let mut venues = ctx.session.lock().import_venues.clone();
    venues.retain(|k, _| k.split_once(':').is_some_and(|(token, _)| dir.join(format!("{token}.csv")).is_file()));
    venues
}

fn render(ctx: &Ctx, status: StatusCode, body: Body) -> Result<Response, WebError> {
    Ok(html(status, Create { ctx, form: matches!(body, Body::Ambiguous { .. } | Body::Warned { .. }), body }.render()?))
}

pub async fn create(Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().map(|u| u.id) else { return Ok(auth::unauthenticated(&ctx)); };
    let format: &'static str = FORMATS.iter().copied().find(|f| Some(*f) == ctx.params.form("format")).unwrap_or(DEFAULT_FORMAT);
    let dir = uploads(&ctx, user);
    // #fetch_upload: the file the unanswered question parked a moment ago, or a fresh upload.
    let (mut parked, text, filename): (Option<String>, Option<Vec<u8>>, Option<String>) = match ctx.params.form("token").filter(|t| token(Some(t))) {
        Some(t) => (Some(t.to_string()), std::fs::read(dir.join(format!("{t}.csv"))).ok(), None),
        None => match &ctx.params.upload { Some((bytes, name)) => (None, Some(bytes.clone()), Some(name.clone())), None => (None, None, None) },
    };
    // #710 7a: Cancel on a question writes nothing and lets the parked upload go.
    if let Some(token) = parked.as_ref().filter(|_| ctx.params.form("cancel").is_some()) {
        release(&ctx, &dir, token);
        return render(&ctx, StatusCode::OK, Body::Refused(ctx.t("tracker.import.cancelled")));
    }
    let Some(text) = text.filter(|t| !blank(t)) else { return render(&ctx, StatusCode::UNPROCESSABLE_ENTITY, Body::Error("tracker.import.error_no_file".into())) };
    // #resolve_offset: a Binance file's zone is the one parked with it, or the one its own name states.
    let offset = match run::offset(format, filename.as_deref(), &text) {
        None => None,
        Some(_) if present(ctx.params.form("token")).is_some() => match parked.as_ref().and_then(|t| parked_offsets(&ctx, &dir).get(t).cloned()) {
            Some(offset) => Some(offset),
            None => return render(&ctx, StatusCode::UNPROCESSABLE_ENTITY, Body::Error("tracker.import.error_unknown_timezone".into())),
        },
        Some(Some(offset)) => Some(offset),
        Some(None) => return render(&ctx, StatusCode::UNPROCESSABLE_ENTITY, Body::Error("tracker.import.error_unknown_timezone".into())),
    };
    // Rule S5: a key answer carries the venue its question offered it for (`<key>~v<exchange>`, `run::route_file`).
    // #710 7a rule N4: an answer's digest (`!…`) rides after the venue.
    let chosen = present(ctx.params.form("api_key_id")).map(|answer| {
        let (body, digest) = answer.rsplit_once('!').map_or((answer, None), |(b, d)| (b, Some(d)));
        let venue = parked.as_ref().filter(|_| body.parse::<i64>().is_ok()).and_then(|t| ctx.session.lock().import_venues.get(&format!("{t}:{body}")).copied());
        let body = venue.map_or_else(|| body.to_string(), |e| format!("{body}~v{e}"));
        digest.map_or(body.clone(), |d| format!("{body}!{d}"))
    });
    // The upload's token, which every answer of its questions is bound to (rule N4).
    let subject = parked.clone().unwrap_or_else(|| format!("{:016x}", rand::random::<u64>()));
    let (bytes, at, written_offset, written_subject) = (text.clone(), ctx.app.now(), offset.clone(), subject.clone());
    // Read on a blocking thread of its own; the shared connection is taken only for the write.
    let file = tokio::task::spawn_blocking(move || run::read(format, &bytes, written_offset.as_deref())).await.map_err(|e| WebError::Config(format!("reading an import: {e}")))?;
    let answer = match file {
        Ok(file) => { let cipher = ctx.app.cipher.clone(); ctx.app.db(move |c| run::write(c, &cipher, user, format, file, &written_subject, chosen.as_deref(), at).map_err(|e| WebError::Config(e.0))).await? }
        Err(answer) => answer,
    };
    if matches!(answer, Answer::Ambiguous(_) | Answer::Warned(..)) {
        // #park
        let token = parked.get_or_insert(subject).clone();
        std::fs::create_dir_all(&dir).map_err(|e| WebError::Config(format!("parking an import: {e}")))?;
        std::fs::write(dir.join(format!("{token}.csv")), &text).map_err(|e| WebError::Config(format!("parking an import: {e}")))?;
        if let Some(offset) = &offset {
            let mut offsets = parked_offsets(&ctx, &dir);
            offsets.insert(token.clone(), offset.clone());
            ctx.session.lock().import_offsets = offsets;
        }
        if let Answer::Ambiguous(choices) | Answer::Warned(_, choices) = &answer {
            let mut venues = parked_venues(&ctx, &dir);
            for c in choices { if let (Some(key), Some(venue)) = (c.key, c.venue) { venues.insert(format!("{token}:{key}"), venue); } }
            if venues != ctx.session.lock().import_venues { ctx.session.lock().import_venues = venues; }
        }
        sweep(&dir);
    } else if let Some(token) = &parked {
        release(&ctx, &dir, token);
    }
    match answer {
        Answer::Written { imported, generic, unrouted, .. } if imported > 0 => {
            // Prepended AND redirected, in that order: #flash is data-turbo-permanent, so a visit keeps it.
            let mut notice = i18n::text(ctx.locale, "tracker.import.imported", &[("count", i18n::Arg::Count(imported as i64))]);
            // D-IMPORT-WARNINGS: a KuCoin file's entries it could only label generically, in one more line.
            if generic > 0 { notice = format!("{notice} {}", i18n::text(ctx.locale, "tracker.import.generic", &[("count", i18n::Arg::Count(generic as i64))])); }
            // D-IMPORT-REPORTS-UNROUTED (#710 7a §3): rows naming an account this install lacks are said to be left out, not dropped silently.
            if !unrouted.is_empty() { notice = format!("{notice} {}", i18n::text(ctx.locale, "tracker.import.unrouted", &[("exchanges", i18n::Arg::Text(&unrouted.join(", ")))])); }
            let markup = flash::render(&flash::take(&ctx.session, &[(flash::NOTICE, notice)]))?;
            let body = turbo::prepend_flash(markup.strip_prefix('\n').unwrap_or(&markup).trim_end()) + &turbo::redirect(&ctx.path("/tracker"));
            Ok(([(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], body).into_response())
        }
        Answer::Written { skipped, unrouted, unrecognised, .. } => render(&ctx, StatusCode::OK, Body::NothingNew {
            skipped: (skipped > 0).then(|| ctx.t_count("tracker.import.skipped", skipped as i64)),
            unrouted: (!unrouted.is_empty()).then(|| i18n::t(ctx.locale, "tracker.import.unrouted", &[("exchanges", i18n::Arg::Text(&unrouted.join(", ")))])),
            unrecognised: (!unrecognised.is_empty()).then(|| i18n::t(ctx.locale, "tracker.import.unrecognised", &[("operations", i18n::Arg::Text(&unrecognised.join(", ")))])),
        }),
        Answer::Refused(reason) => {
            // The reason names one of our own checks, not Binance's answer: the log keeps it, the user reads what to do.
            crate::engine::log(&format!("[Binance] import refused: {reason}"));
            render(&ctx, StatusCode::UNPROCESSABLE_ENTITY, Body::Refused(i18n::text(ctx.locale, refused_key(&reason), &[])))
        }
        Answer::Warned(warnings, choices) => {
            use run::{Accept, Warning};
            let lines = warnings.iter().map(|w| match w {
                Warning::HeldElsewhere(a) => i18n::text(ctx.locale, "tracker.import.warn_held", &[("account", i18n::Arg::Text(a))]),
                Warning::OtherAccount(a) => i18n::text(ctx.locale, "tracker.import.warn_other", &[("account", i18n::Arg::Text(a))]),
                Warning::Existing(n, a) => i18n::text(ctx.locale, "tracker.import.warn_existing", &[("count", i18n::Arg::Count(*n as i64)), ("account", i18n::Arg::Text(a))]),
                Warning::Unrouted(n) => i18n::text(ctx.locale, "tracker.import.warn_unrouted", &[("count", i18n::Arg::Count(*n as i64))]),
                Warning::Retained => i18n::text(ctx.locale, "tracker.import.warn_retained", &[]),
                Warning::Changed => i18n::text(ctx.locale, "tracker.import.warn_changed", &[]),
            }).collect();
            let options = choices.iter().enumerate().map(|(i, k)| SegmentedOption { value: k.value.clone().into(), active: i == 0, href: None, label: match k.accept {
                Some(Accept::To) => i18n::text(ctx.locale, "tracker.import.import_to", &[("account", i18n::Arg::Text(&k.label))]),
                Some(Accept::New) => ctx.t("tracker.import.import_new"),
                Some(Accept::Rest) => ctx.t("tracker.import.import_rest"),
                Some(Accept::Anyway) => ctx.t("tracker.import.import_anyway"),
                Some(Accept::Here) | None => ctx.t("tracker.import.import_here"),
            } }).collect();
            let accounts = Segmented { fluid: true, label: ctx.t("tracker.import.account"), key: None, options, links: false, frame: None }.render()?;
            render(&ctx, StatusCode::OK, Body::Warned { csrf: ctx.csrf_token(), token: parked.unwrap_or_default(), format, lines, accounts,
                                                        first: choices.first().map(|k| k.value.clone()).unwrap_or_default() })
        }
        Answer::Error(error) => render(&ctx, StatusCode::UNPROCESSABLE_ENTITY, Body::Error(format!("tracker.import.error_{error}"))),
        Answer::Ambiguous(keys) => {
            let options = keys.iter().enumerate().map(|(i, k)| SegmentedOption { value: k.value.clone().into(), label: if k.new { ctx.t("tracker.import.new_account") } else { k.label.clone() },
                                                                                 active: i == 0, href: None }).collect();
            let accounts = Segmented { fluid: true, label: ctx.t("tracker.import.account"), key: None, options, links: false, frame: None }.render()?;
            render(&ctx, StatusCode::OK, Body::Ambiguous { csrf: ctx.csrf_token(), token: parked.unwrap_or_default(), format, accounts,
                                                           first: keys.first().map(|k| k.value.clone()).unwrap_or_default(),
                                                           // The venue question keeps 2.x's text; the account question (#710 7a §3) has its own.
                                                           question: if keys.iter().any(|k| k.new) { "tracker.import.which_of_accounts" } else { "tracker.import.which_account" } })
        }
    }
}

/// #release: the parked upload `token`, its zone and its venues go.
fn release(ctx: &Ctx, dir: &std::path::Path, token: &str) {
    let _ = std::fs::remove_file(dir.join(format!("{token}.csv")));
    let mut offsets = parked_offsets(ctx, dir);
    offsets.remove(token);
    ctx.session.lock().import_offsets = offsets;
    let mut venues = parked_venues(ctx, dir);
    venues.retain(|k, _| !k.starts_with(&format!("{token}:")));
    if venues != ctx.session.lock().import_venues { ctx.session.lock().import_venues = venues; }
}

/// #sweep_old_uploads: parked files older than UPLOAD_TTL go.
fn sweep(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "csv") && entry.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age > UPLOAD_TTL) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The stored-history checks (`binance_writer::csv_refusal`): the saved history needs its repair. Every other refusal is
/// the file's: cut short, edited or not this account's.
fn refused_key(reason: &str) -> &'static str {
    const HISTORY: [&str; 9] = ["unreadable identity metadata", "conflicting bucket digest", "unsupported bucket", "missing retained identity",
        "duplicate or conflicting retained digest", "suppression has no retained reason", "unreadable retained identity evidence",
        "CSV alias points to deleted row", "CSV group is already occupied"];
    if HISTORY.contains(&reason) { "tracker.import.identity_refused_history" } else { "tracker.import.identity_refused_file" }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_stored_history_refusal_asks_for_the_repair_and_every_other_for_a_fresh_export() {
        assert_eq!(super::refused_key("missing retained identity"), "tracker.import.identity_refused_history");
        assert_eq!(super::refused_key("CSV group is already occupied"), "tracker.import.identity_refused_history");
        assert_eq!(super::refused_key("partial CSV dust pair"), "tracker.import.identity_refused_file");
        assert_eq!(super::refused_key("exported identity has no retained binding"), "tracker.import.identity_refused_file");
    }

    #[test]
    fn blank_and_token_follow_v2() {
        assert!(super::blank(b"") && super::blank(b" \r\n\t\x0b\x0c"));
        assert!(!super::blank(b"\xef\xbb\xbf") && !super::blank(b"a"));
        assert!(super::token(Some("0123456789abcdef")));
        assert!(!super::token(Some("0123456789ABCDEF")) && !super::token(Some("0123456789abcde")) && !super::token(None));
    }
}
