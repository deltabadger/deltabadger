//! Tracker::ImportsController: the dialog, and a file read into the ledger through the shared import operation
//! (tracker::import::service). A request is classified from what it carries, before any field is dropped (PLAN-710-confirm
//! rules N2/P1/Q4): a fresh upload (a file, no token, no handle, no cancel), a confirmation (one token, one handle) or a
//! Cancel (one token); anything else is no file. The web renders the operation's `Outcome`.
use super::{auth, Ctx, WebError};
use crate::tracker::import::run::{self, Answer};
use crate::tracker::import::service::{self, Outcome};
use crate::web::bots::{Segmented, SegmentedOption};
use crate::web::{flash, i18n, turbo};
use askama::Template;
use axum::{extract::Extension, http::{header, StatusCode}, response::{IntoResponse, Response}};

pub use run::{DEFAULT_FORMAT, FORMATS};

#[derive(Template)]
#[template(path = "tracker/import_new.html")]
struct New<'a> { ctx: &'a Ctx, csrf: String, formats: String, default_format: &'static str }

/// What create.html.erb says when no rows landed.
enum Body {
    Error(String),
    Refused(String),
    /// 2.x's venue question (Binance or Binance.US): its selector, as recorded.
    Ambiguous { csrf: String, token: String, format: &'static str, accounts: String, first: String },
    /// #710 8a: a venue export's question (the account question, or what writing it warns about): the question's lines,
    /// one named button per destination with its own warnings, and Cancel.
    Choose { csrf: String, token: String, format: &'static str, lines: Vec<String>, buttons: Vec<Button> },
    /// #710 7a/8a: what a Deltabadger CSV's import says before writing, its one sealed answer (`value`, posted by the
    /// `label` button), and Cancel.
    Warned { csrf: String, token: String, format: &'static str, lines: Vec<String>, value: String, label: String },
    NothingNew { skipped: Option<String>, unrouted: Option<String>, unrecognised: Option<String> },
}

/// A destination's button: its sealed answer, its name (markup) and the warnings its click accepts (markup).
struct Button { value: String, html: String, notes: Vec<String> }

/// The lines a warning shows, as markup (an account named in each line its own).
fn lines(locale: &str, w: &run::Warning) -> Vec<String> {
    use run::Warning;
    let named = |key: &str, label: &crate::identity::Label| i18n::t(locale, key, &[("account", i18n::Arg::Html(&label.html()))]);
    let counted = |key: &str, label: &crate::identity::Label, n: usize| i18n::t(locale, key, &[("account", i18n::Arg::Html(&label.html())), ("count", i18n::Arg::Count(n as i64))]);
    match w {
        Warning::HeldElsewhere(a) => vec![named("tracker.import.warn_held_html", a)],
        Warning::OtherAccount(a) => vec![named("tracker.import.warn_other_html", a)],
        Warning::Existing(per) => per.iter().map(|(a, n)| counted("tracker.import.warn_existing_html", a, *n)).collect(),
        Warning::Destination(per) => per.iter().map(|(a, n)| counted("tracker.import.destination_html", a, *n)).collect(),
        Warning::Twice(n) => vec![i18n::t(locale, "tracker.import.twice_html", &[("count", i18n::Arg::Count(*n as i64))])],
        Warning::Unrouted(n) => vec![i18n::t(locale, "tracker.import.warn_unrouted", &[("count", i18n::Arg::Count(*n as i64))])],
        Warning::Retained => vec![i18n::t(locale, "tracker.import.warn_retained", &[])],
        Warning::Dust => vec![i18n::t(locale, "tracker.import.warn_dust", &[])],
        Warning::Changed => vec![i18n::t(locale, "tracker.import.warn_changed", &[])],
        Warning::LeftOut(names) => vec![i18n::t(locale, "tracker.import.rows_left_out_named_html", &[("exchanges", i18n::Arg::Text(&names.join(", ")))])],
        Warning::Undone(n, accounts) => {
            let named = accounts.iter().map(crate::identity::Label::html).collect::<Vec<_>>().join(", ");
            vec![i18n::t(locale, "tracker.import.links_undone_html", &[("count", i18n::Arg::Count(*n as i64)), ("accounts", i18n::Arg::Html(&named))])]
        }
    }
}

/// #710 8a: one named button per destination ("Import to KuCoin² (rows in file: 12)").
fn buttons(locale: &str, choices: &[run::Choice]) -> Vec<Button> {
    choices.iter().map(|k| {
        let key = if k.new || k.accept == Some(run::Accept::New) { "tracker.import.import_new_html" } else { "tracker.import.import_to_html" };
        Button { value: k.value.clone(), html: i18n::t(locale, key, &[("account", i18n::Arg::Html(&k.shown.html())), ("count", i18n::Arg::Count(k.count as i64))]),
                 notes: k.notes.iter().flat_map(|w| lines(locale, w)).collect() }
    }).collect()
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

/// What a request is (rules N2/P1/Q4/L7): read from every occurrence in the query, the body's fields and its file parts.
#[derive(Debug, PartialEq)]
enum Input { Fresh, Confirm(String, String), Cancel(String), NoFile }

fn classify(params: &crate::web::Params) -> Input {
    let all = || params.query.iter().chain(params.form.iter());
    let values = |name: &'static str| all().filter(move |(k, _)| k == name).map(|(_, v)| v.as_str());
    let file_part = |name: &str| params.file_parts.iter().any(|p| p == name);
    let tokens: std::collections::BTreeSet<&str> = values("token").collect();
    // An `api_key_id` shaped as a handle is one; a bare one on a fresh upload is ignored (the recorded `999`).
    let handles: std::collections::BTreeSet<&str> = values("handle").chain(values("api_key_id").filter(|v| service::handle_shaped(v))).collect();
    let cancel = values("cancel").next().is_some() || file_part("cancel");
    if file_part("token") || file_part("handle") { return Input::NoFile; }
    if tokens.is_empty() && handles.is_empty() && !cancel { return Input::Fresh; }
    // Not fresh: one token and at most one choice, whatever else is attached.
    let tokens: Vec<&str> = tokens.into_iter().collect();
    let ([token], true) = (tokens.as_slice(), handles.len() <= 1) else { return Input::NoFile };
    if !service::token_shaped(token) { return Input::NoFile; }
    // Cancel wins over the form's own hidden handle.
    if cancel { return Input::Cancel(token.to_string()); }
    match handles.into_iter().next() { Some(handle) => Input::Confirm(token.to_string(), handle.to_string()), None => Input::NoFile }
}

fn render(ctx: &Ctx, status: StatusCode, body: Body) -> Result<Response, WebError> {
    Ok(html(status, Create { ctx, form: matches!(body, Body::Ambiguous { .. } | Body::Choose { .. } | Body::Warned { .. }), body }.render()?))
}

pub async fn create(Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().map(|u| u.id) else { return Ok(auth::unauthenticated(&ctx)); };
    let format: &'static str = FORMATS.iter().copied().find(|f| Some(*f) == ctx.params.form("format")).unwrap_or(DEFAULT_FORMAT);
    let view = ctx.clone();
    let signed_in: service::SignedIn = std::sync::Arc::new(move |c: &rusqlite::Connection| auth::still_signed_in(c, &view).map_err(|e| crate::sync::SyncError(format!("{e:?}"))));
    let outcome = match classify(&ctx.params) {
        Input::NoFile => Outcome::NoFile,
        Input::Fresh => match &ctx.params.upload {
            Some((bytes, name)) if !blank(bytes) => service::upload(&ctx.app, user, format, bytes.clone(), Some(name.clone()), signed_in).await?,
            _ => Outcome::NoFile,
        },
        Input::Confirm(token, handle) => service::confirm(&ctx.app, user, token, handle, signed_in).await?,
        Input::Cancel(token) => service::cancel(&ctx.app, user, token, signed_in).await?,
    };
    respond(&ctx, outcome)
}

/// An `Outcome`, as create.html.erb (and 8a's questions) render it.
fn respond(ctx: &Ctx, outcome: Outcome) -> Result<Response, WebError> {
    match outcome {
        // The sign-in ended while the file was read: nothing written, the request is signed out.
        Outcome::SignedOut => Ok(auth::signed_out(ctx)),
        Outcome::NoFile => render(ctx, StatusCode::UNPROCESSABLE_ENTITY, Body::Error("tracker.import.error_no_file".into())),
        // #710 7a: Cancel on a question writes nothing and lets the parked upload go.
        Outcome::Cancelled => render(ctx, StatusCode::OK, Body::Refused(ctx.t("tracker.import.cancelled"))),
        Outcome::Written { imported, generic, unrouted, .. } if imported > 0 => {
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
        Outcome::Written { skipped, unrouted, unrecognised, .. } => render(ctx, StatusCode::OK, Body::NothingNew {
            skipped: (skipped > 0).then(|| ctx.t_count("tracker.import.skipped", skipped as i64)),
            unrouted: (!unrouted.is_empty()).then(|| i18n::t(ctx.locale, "tracker.import.unrouted", &[("exchanges", i18n::Arg::Text(&unrouted.join(", ")))])),
            unrecognised: (!unrecognised.is_empty()).then(|| i18n::t(ctx.locale, "tracker.import.unrecognised", &[("operations", i18n::Arg::Text(&unrecognised.join(", ")))])),
        }),
        Outcome::Refused(reason) => {
            // The reason names one of our own checks, not Binance's answer: the log keeps it, the user reads what to do.
            crate::engine::log(&format!("[Binance] import refused: {reason}"));
            render(ctx, StatusCode::UNPROCESSABLE_ENTITY, Body::Refused(i18n::text(ctx.locale, refused_key(&reason), &[])))
        }
        Outcome::Error(error) => render(ctx, StatusCode::UNPROCESSABLE_ENTITY, Body::Error(format!("tracker.import.error_{error}"))),
        Outcome::Question { token, format, answer, .. } => question(ctx, token, format, answer),
    }
}

/// A question's page: the selector, the named buttons, or a Deltabadger CSV's one sealed answer; each value a handle.
fn question(ctx: &Ctx, token: String, format: &'static str, answer: Answer) -> Result<Response, WebError> {
    match answer {
        Answer::Warned(warnings, choices) => {
            use run::{Accept, Warning};
            let csrf = ctx.csrf_token();
            match choices.as_slice() {
                // A Deltabadger CSV: every line, one sealed answer.
                [only] if matches!(only.accept, Some(Accept::Anyway | Accept::Rest | Accept::Import)) => {
                    let label = ctx.t(match only.accept { Some(Accept::Rest) => "tracker.import.import_rest", Some(Accept::Anyway) => "tracker.import.import_anyway", _ => "tracker.import.confirm" });
                    render(ctx, StatusCode::OK, Body::Warned { csrf, token, format, lines: warnings.iter().flat_map(|w| lines(ctx.locale, w)).collect(), value: only.value.clone(), label })
                }
                // A venue export: what is not one destination's above the buttons; each destination's own under its button.
                _ => render(ctx, StatusCode::OK, Body::Choose { csrf, token, format, buttons: buttons(ctx.locale, &choices),
                                                               lines: warnings.iter().filter(|w| matches!(w, Warning::Changed | Warning::Retained | Warning::Dust | Warning::Undone(..))).flat_map(|w| lines(ctx.locale, w)).collect() }),
            }
        }
        // The account question (#710 7a §3, 8a): its own text, one named button per account and the new one.
        Answer::Ambiguous(keys) if keys.iter().any(|k| k.new) => render(ctx, StatusCode::OK, Body::Choose {
            csrf: ctx.csrf_token(), token, format, lines: vec![ctx.t("tracker.import.which_of_accounts")], buttons: buttons(ctx.locale, &keys) }),
        // The venue question keeps 2.x's selector and text.
        Answer::Ambiguous(keys) => {
            let options = keys.iter().enumerate().map(|(i, k)| SegmentedOption { value: k.value.clone().into(), label: k.shown.plain(), active: i == 0, href: None }).collect();
            let accounts = Segmented { fluid: true, label: ctx.t("tracker.import.account"), key: None, options, links: false, frame: None }.render()?;
            render(ctx, StatusCode::OK, Body::Ambiguous { csrf: ctx.csrf_token(), token, format, accounts, first: keys.first().map(|k| k.value.clone()).unwrap_or_default() })
        }
        _ => render(ctx, StatusCode::UNPROCESSABLE_ENTITY, Body::Error("tracker.import.error_no_file".into())),
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
        use crate::tracker::import::service::token_shaped;
        assert!(super::blank(b"") && super::blank(b" \r\n\t\x0b\x0c"));
        assert!(!super::blank(b"\xef\xbb\xbf") && !super::blank(b"a"));
        assert!(token_shaped("0123456789abcdef"));
        assert!(!token_shaped("0123456789ABCDEF") && !token_shaped("0123456789abcde"));
    }

    /// PLAN-710-confirm rules N2/P1/Q4/L7: a request is fresh only with no token, no handle and no cancel anywhere; one token
    /// and one choice, else no file; Cancel over the form's hidden handle.
    #[test]
    fn a_request_is_classified_from_everything_it_carries() {
        use super::Input;
        let (t, u) = ("0123456789abcdef", "fedcba9876543210");
        let h = |t: &str| format!("{t}.1.0.{}", "a".repeat(32));
        let params = |query: &[(&str, &str)], form: &[(&str, &str)], files: &[&str]| crate::web::Params {
            full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None, json: None, upload: None,
            query: query.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), form: form.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            file_parts: files.iter().map(|f| f.to_string()).collect() };
        let c = |q: &[(&str, &str)], f: &[(&str, &str)], files: &[&str]| super::classify(&params(q, f, files));
        assert_eq!(c(&[], &[("format", "binance"), ("api_key_id", "999")], &[]), Input::Fresh);
        assert_eq!(c(&[], &[("token", t), ("api_key_id", &h(t))], &[]), Input::Confirm(t.into(), h(t)));
        assert_eq!(c(&[], &[("token", t), ("api_key_id", &h(t)), ("cancel", "Cancel")], &[]), Input::Cancel(t.into()));
        assert_eq!(c(&[("cancel", "1"), ("token", t)], &[], &[]), Input::Cancel(t.into()));
        assert_eq!(c(&[("token", t)], &[("format", "binance")], &[]), Input::NoFile, "a query token with a fresh file");
        assert_eq!(c(&[], &[("format", "binance")], &["token"]), Input::NoFile, "a file-valued token part");
        assert_eq!(c(&[], &[("api_key_id", &h(t))], &[]), Input::NoFile, "a handle with no token");
        assert_eq!(c(&[("token", t), ("api_key_id", &h(t))], &[("token", u), ("api_key_id", &h(u))], &[]), Input::NoFile, "two uploads' pairs");
        assert_eq!(c(&[("handle", &h(t))], &[("token", t), ("api_key_id", &h(u))], &[]), Input::NoFile, "two choices");
        assert_eq!(c(&[], &[("token", "0123")], &[]), Input::NoFile);
    }
}
