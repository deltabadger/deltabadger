//! First run: SetupController (GET and POST /setup), the language step in front of it (GET /language, D-LANGUAGE-STEP)
//! and the redirect that ApplicationController puts in front of every page while no admin user exists. Rust serves it on
//! an install `db:prepare` made; creating that install (schema, seeds, secrets) stays with the entrypoint.
//!
//! D-SETUP-NO-CONNECT-CODE: 2.x's fold-out that redeems a pasted connect code (POST /setup/platform_connection) is gone;
//! a subscriber connects later, where the app asks (the wizard's index step, the Connect sheet, Settings). CLAIM_TOKEN's
//! redeem on GET /setup stays.
//!
//! Stricter than 2.x: both admin creation and claim writes recheck for an admin in an IMMEDIATE transaction.
//! Two forms submitted at once cannot make two admins, and a claim in flight cannot change configuration after
//! setup closes. A losing admin creation redirects to the root; a losing claim renders a failure.
use super::layout::{self, Ctx, Page};
use super::{auth, i18n, i18n::Arg, launchpad, locale, App, Params, WebError};
use crate::codec::format_time;
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::Response;
use rusqlite::{Connection, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// User.exists?(admin: true).
pub fn admin_exists(c: &Connection) -> Result<bool, WebError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE admin = 1)", [], |r| r.get(0))?)
}

/// Whether ApplicationController#redirect_to_setup_if_needed stands in front of this request. Everything the
/// pipeline serves is ApplicationController's, except: setup itself and the language step before it; Doorkeeper's authorize page
/// (Oauth::BaseController); the `/settings` route redirect, which the router answers before any controller; and
/// the versioned API (ActionController::API), which this build does not serve. /up, /health-check, /ui-ready,
/// /csp-report, /cable and the OAuth API are outside the pipeline already.
pub fn gated(params: &Params) -> bool {
    let unprefixed = params.path_locale.is_none();
    let path = params.route_path.as_str();
    let setup = unprefixed && (path == "/setup" || path == "/language" || path.strip_prefix("/setup.").is_some_and(|f| !f.contains('/')));
    !(setup || (unprefixed && path == "/oauth/authorize") || path == "/settings" || path == "/api/v1" || path.starts_with("/api/v1/"))
}

/// The request's languages (`Accept-Language`, every such header in order) under the language step's rule.
pub fn first_choice(headers: &HeaderMap) -> locale::Choice {
    let header: Vec<&str> = headers.get_all(header::ACCEPT_LANGUAGE).iter().filter_map(|value| value.to_str().ok()).collect();
    locale::first_choice(&locale::accept_language(&header.join(",")))
}

/// D-LANGUAGE-STEP: where a first visit goes. A browser in one of our languages but English goes straight to the form in
/// it; an English one, or one in a language we don't have, to the language step.
fn onboarding_path(headers: &HeaderMap) -> String {
    match first_choice(headers) {
        locale::Choice::Use(code) => format!("/setup?locale={code}"),
        locale::Choice::Ask(_) => "/language".into(),
    }
}

/// The redirect itself: before set_no_cache, so Rack's `no-cache` even for a signed-in user, and before switch_locale.
/// 2.x's goes to /setup; this build's to the first onboarding page (`onboarding_path`).
pub fn to_setup(headers: &HeaderMap) -> Response {
    layout::early_redirect(StatusCode::FOUND, &onboarding_path(headers))
}

/// `new_setup_path` under default_url_options: the locale rides in the query, unless it is the default.
fn setup_path(ctx: &Ctx) -> String {
    if ctx.locale == locale::DEFAULT { "/setup".into() } else { format!("/setup?locale={}", ctx.locale) }
}

/// SETUP_TOKEN compared as ActiveSupport::SecurityUtils.secure_compare does: SHA-256 digests in constant time, so
/// neither the length nor any prefix of the value leaks through timing. A blank expected value matches nothing.
pub fn token_matches(given: &str, expected: &str) -> bool {
    !crate::v2::blank(expected) && bool::from(Sha256::digest(given.as_bytes()).ct_eq(&Sha256::digest(expected.as_bytes())))
}

/// Entry has already applied 2.x's shallow, query-winning merge for every setup route.
fn parameter(params: &Params, name: &str) -> serde_json::Value {
    params.json.as_ref().and_then(|parameters| parameters.get(name)).cloned().unwrap_or(serde_json::Value::Null)
}

/// SetupController#require_setup_token: `Some(answer)` when the request stops here, the locked page or the
/// redirect that takes the token out of the URL. Without SETUP_TOKEN in the environment nothing happens.
fn require_setup_token(ctx: &Ctx, headers: &HeaderMap) -> Result<Option<Response>, WebError> {
    let Some(expected) = ctx.app.config.setup_token.as_deref() else { return Ok(None) };
    // params[:token].presence: a blank merged value falls back to the session.
    let token = parameter(&ctx.params, "token");
    let param = match token {
        serde_json::Value::Null | serde_json::Value::Bool(false) => None,
        serde_json::Value::String(s) => (!crate::v2::blank(&s)).then_some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(true) => Some("true".into()),
        serde_json::Value::Array(a) if a.is_empty() => None,
        serde_json::Value::Object(o) if o.is_empty() => None,
        // A supplied structured token must fail, not fall back to an authorized session.
        _ => Some(String::new()),
    };
    let given = param.clone().or_else(|| ctx.session.lock().setup_token.clone()).unwrap_or_default();
    if !token_matches(&given, expected) {
        let body = LockedView { v: ctx, languages: languages(ctx) }.render()?;
        return page(ctx, StatusCode::FORBIDDEN, body, Vec::new()).map(Some);
    }
    ctx.session.lock().setup_token = Some(given);
    let read = matches!(ctx.method, Method::GET | Method::HEAD);
    // D-LANGUAGE-STEP: a request that named one of our locales goes to the form in it, as in 2.x; any other goes the way
    // the gate goes (the token is in the session by now).
    let to = if ctx.params.locale().and_then(locale::known).is_some() { setup_path(ctx) } else { onboarding_path(headers) };
    Ok((read && param.is_some()).then(|| layout::redirect(StatusCode::FOUND, &to)))
}

/// The two filters in front of every setup action: an existing admin closes setup, then the token gate.
async fn filters(app: &App, ctx: &Ctx, headers: &HeaderMap) -> Result<Option<Response>, WebError> {
    if app.db(admin_exists).await? {
        return Ok(Some(layout::redirect(StatusCode::FOUND, &ctx.path("/"))));
    }
    require_setup_token(ctx, headers)
}

/// String#upcase_first.
pub(super) fn upcase_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// layouts/_language_dropdown_full.html.erb's links: the setup routes have no locale segment, so
/// locale_switch_path keeps the requested path and puts the locale in the query, beside the page's own (reserved keys and credentials dropped), sorted as to_query sorts.
fn languages(ctx: &Ctx) -> Vec<layout::Language> {
    let query = locale::switch_query(&ctx.params.query);
    ctx.languages_to(|code| {
        let own = format!("locale={code}");
        let mut entries: Vec<&str> = query.split('&').filter(|entry| !entry.is_empty()).chain([own.as_str()]).collect();
        entries.sort_unstable();
        format!("{}?{}", ctx.params.route_path, entries.join("&"))
    })
}

/// SetupController renders in the devise layout whatever frame asked (`layout 'devise'` overrides turbo-rails'
/// frame layout).
fn page(ctx: &Ctx, status: StatusCode, body: String, flash_now: Vec<(&str, String)>) -> Result<Response, WebError> {
    onboarding_page(ctx, status, body, flash_now, "")
}

/// `page` for one of the two onboarding pages (D-DEVISE-STABLE-TOP): `name` is `setup` or `language`.
fn onboarding_page(ctx: &Ctx, status: StatusCode, body: String, flash_now: Vec<(&str, String)>, name: &str) -> Result<Response, WebError> {
    let full = Ctx { turbo_frame: None, ..ctx.clone() };
    let csrf = full.csrf_token();
    layout::onboarding(&full, &csrf, Page { status, body, flash_now }, name)
}

#[derive(Template)]
#[template(path = "setup/locked.html")]
struct LockedView<'a> {
    v: &'a Ctx,
    languages: Vec<layout::Language>,
}

/// The "Connected ✓" line's state: AppConfig.platform_connected? and, when connected, platform_proxies_configured?.
struct Platform {
    connected: bool,
    proxies: bool,
}

fn platform(c: &Connection, app: &App) -> Result<Platform, WebError> {
    let connected = auth::app_config(c, &app.cipher, "platform_connected_at")?.is_some_and(|at| !crate::v2::blank(&at));
    let proxies = c.query_row("SELECT EXISTS(SELECT 1 FROM app_configs WHERE substr(key, 1, 6) = 'proxy_')", [], |r| r.get(0))?;
    Ok(Platform { connected, proxies })
}

#[derive(Template)]
#[template(path = "setup/new.html")]
struct NewView<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    errors: &'a [FieldError],
    /// 2.x's inline form errors on the password field: its messages, joined, first letter up.
    password_error: Option<String>,
    email: &'a str,
    platform: &'a Platform,
}

async fn read_platform(app: &App) -> Result<Platform, WebError> {
    let inner = app.clone();
    app.db(move |c| platform(c, &inner)).await
}

struct Form<'a> {
    status: StatusCode,
    errors: &'a [FieldError],
    email: &'a str,
    flash_now: Vec<(&'a str, String)>,
}

async fn form(app: &App, ctx: &Ctx, form: Form<'_>) -> Result<Response, WebError> {
    let platform = read_platform(app).await?;
    let csrf = ctx.csrf_token();
    let password: Vec<&str> = form.errors.iter().filter(|e| e.attribute == "password").map(|e| e.message.as_str()).collect();
    let password_error = (!password.is_empty()).then(|| upcase_first(&password.join(", ")));
    let body = NewView { v: ctx, csrf: &csrf, errors: form.errors, password_error, email: form.email, platform: &platform }.render()?;
    onboarding_page(ctx, form.status, body, form.flash_now, "setup")
}

/// Platform::RedeemClaim: the claim, then in one transaction the market-data configuration, the proxies, the
/// connection time and a pending sync, then Setup::SeedAndSyncJob. `Ok(Err(sentence))` is a failed claim. `setup`: setup's
/// claim (CLAIM_TOKEN's, on GET /setup), refused at commit once an admin exists; the admin's own claim in settings is not.
pub(crate) async fn redeem(app: &App, code: serde_json::Value, now: chrono::DateTime<chrono::Utc>, setup: bool) -> Result<Result<Option<String>, String>, WebError> {
    let claim = match launchpad::claim(&app.config.launchpad_url, code).await {
        Ok(claim) => claim,
        Err(launchpad::Failure::Message(sentence)) => return Ok(Err(sentence)),
        Err(launchpad::Failure::Malformed) => return Err(WebError::Config("the platform's claim answer has proxies that are not an object".into())),
    };
    let inner = app.clone();
    let identity = claim.identity.clone();
    let written = app.db(move |c| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        // The upstream request may have overlapped admin creation. Setup must still be open at commit.
        if setup && admin_exists(&tx)? { return Ok(false); }
        let mut writes = vec![("market_data_url".to_string(), claim.market_url), ("market_data_token".to_string(), claim.market_token),
                              ("market_data_provider".to_string(), "deltabadger".to_string())];
        writes.extend(claim.proxies);
        writes.push(("platform_connected_at".into(), now.format("%Y-%m-%dT%H:%M:%SZ").to_string()));
        writes.push(("setup_sync_status".into(), "pending".into()));
        for (key, value) in &writes {
            crate::app_config::set(&tx, &inner.cipher, key, value, now).map_err(|e| WebError::Config(format!("platform claim: {e}")))?;
        }
        tx.commit()?;
        // Setup::SeedAndSyncJob.perform_later, after the commit.
        if let Ok(jobs) = inner.job_wakers() { jobs.wake(crate::jobs::setup_sync::NAME, None, None); }
        Ok(true)
    }).await?;
    if !written { return Ok(Err("Setup is already complete. The connection was not changed.".into())); }
    Ok(Ok(identity))
}

/// The setup action for `/setup.<format>`: 2.x routes the format to the same action, and its filters answer as
/// for `/setup` (the locked page is HTML whatever the format). HTML dispatches to new/create. Any
/// other single-segment path is this build's refusal, as without this route.
pub async fn formatted(State(app): State<App>, Extension(ctx): Extension<Ctx>, headers: HeaderMap) -> Result<Response, WebError> {
    if ctx.params.path_locale.is_some() || !ctx.params.route_path.starts_with("/setup.") {
        return Ok(layout::not_ported_for(&ctx));
    }
    if ctx.params.route_path == "/setup.html" {
        match ctx.method {
            Method::GET | Method::HEAD => return new(State(app), Extension(ctx), headers).await,
            Method::POST => return create(State(app), Extension(ctx), headers).await,
            _ => {}
        }
    }
    if let Some(answer) = filters(&app, &ctx, &headers).await? {
        return Ok(answer);
    }
    Ok(layout::refused(&ctx, "setup is served as HTML only"))
}

/// GET /setup. The identity of a claim redeemed earlier in this session prefills the form; while the install is not
/// connected, CLAIM_TOKEN is redeemed here (a failure is shown and does not block setup).
pub async fn new(State(app): State<App>, Extension(ctx): Extension<Ctx>, headers: HeaderMap) -> Result<Response, WebError> {
    if let Some(answer) = filters(&app, &ctx, &headers).await? {
        return Ok(answer);
    }
    let mut email = ctx.session.lock().platform_identity.clone();
    let mut flash_now = Vec::new();
    if let Some(code) = app.config.claim_token.as_deref() {
        if !read_platform(&app).await?.connected {
            match redeem(&app, serde_json::Value::String(code.to_string()), ctx.now, true).await? {
                Ok(identity) => {
                    ctx.session.lock().platform_identity = identity.clone();
                    email = identity.or(email);
                }
                Err(sentence) => flash_now.push((super::flash::ALERT, sentence)),
            }
        }
    }
    form(&app, &ctx, Form { status: StatusCode::OK, errors: &[], email: email.as_deref().unwrap_or(""), flash_now }).await
}

/// What the account form submitted: `params.require(:user).permit(:email, :password)` (D-NO-USER-NAME: no name). `None` when the
/// form has no `user` at all (ActionController::ParameterMissing).
struct Submitted {
    email: Option<String>,
    password: Option<String>,
}

fn submitted(params: &Params) -> Option<Submitted> {
    let parameters = params.json.as_ref()?;
    let user = parameters.get("user")?.as_object().filter(|user| !user.is_empty())?;
    // Strong parameters keep only these scalar attributes; ActiveModel casts booleans to t/f.
    let field = |name: &str| match user.get(name)? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(if *b { "t" } else { "f" }.into()),
        _ => None,
    };
    Some(Submitted { email: field("email"), password: field("password") })
}

/// One error of the new user: its attribute and either an ActiveModel message type (with `count`) or a message
/// the model wrote itself (already translated, as `errors.add(:name, I18n.t(...))` does).
enum Message {
    Kind(&'static str, Option<i64>),
    Text(String),
}

/// User.human_attribute_name.
fn attribute_name(locale: &str, attribute: &str) -> String {
    let keys = [format!("activerecord.attributes.user.{attribute}"), format!("attributes.{attribute}")];
    i18n::text_chain(locale, &keys, &[]).unwrap_or_else(|| {
        let mut chars = attribute.replace('_', " ").chars().collect::<Vec<_>>();
        if let Some(first) = chars.first_mut() { *first = first.to_ascii_uppercase(); }
        chars.into_iter().collect()
    })
}

/// `errors[attribute]`'s message for an error of `kind` (Devise's validatable on a password change). A confirmation
/// error names the password, as `validates_confirmation_of` passes `attribute: human_attribute_name(:password)`.
pub(super) fn kind_message(locale: &str, attribute: &str, kind: &'static str, count: Option<i64>) -> String {
    if kind == "confirmation" {
        let name = attribute_name(locale, "password");
        let keys = [format!("activerecord.errors.models.user.attributes.{attribute}.confirmation"), "activerecord.errors.messages.confirmation".into(),
                    format!("errors.attributes.{attribute}.confirmation"), "errors.messages.confirmation".into()];
        return i18n::text_chain(locale, &keys, &[("attribute", Arg::Text(&name))]).unwrap_or_else(|| format!("Translation missing: {locale}.errors.messages.confirmation"));
    }
    full_message(locale, attribute, &Message::Kind(kind, count)).0
}

/// The message and ActiveModel::Error#full_message: `errors.format` around the attribute's name and the message, the message
/// looked up along ActiveModel's chain for a type.
fn full_message(locale: &str, attribute: &str, message: &Message) -> (String, String) {
    let name = attribute_name(locale, attribute);
    let text = match message {
        Message::Text(text) => text.clone(),
        Message::Kind(kind, count) => {
            let keys = [format!("activerecord.errors.models.user.attributes.{attribute}.{kind}"), format!("activerecord.errors.models.user.{kind}"),
                        format!("activerecord.errors.messages.{kind}"), format!("errors.attributes.{attribute}.{kind}"), format!("errors.messages.{kind}")];
            let mut args = vec![("attribute", Arg::Text(&name)), ("model", Arg::Text("User"))];
            if let Some(n) = count { args.push(("count", Arg::Count(*n))); }
            i18n::text_chain(locale, &keys, &args).unwrap_or_else(|| format!("Translation missing: {locale}.errors.messages.{kind}"))
        }
    };
    let full = i18n::text_chain(locale, &["errors.format".into()], &[("attribute", Arg::Text(&name)), ("message", Arg::Text(&text))])
        .unwrap_or_else(|| format!("{name} {text}"));
    (text, full)
}

/// A User error's full message (`errors.full_messages`) for a message of a type, as `inclusion`.
pub(crate) fn user_full_message(locale: &str, attribute: &str, kind: &'static str) -> String {
    full_message(locale, attribute, &Message::Kind(kind, None)).1
}

/// Devise's email normalisation before validation: String#strip, then downcase.
pub fn normalized_email(email: &str) -> String {
    crate::v2::strip(email).to_lowercase()
}

/// One validation error of the new admin, in the order 2.x adds them.
pub struct FieldError {
    pub attribute: &'static str,
    /// The message alone, as `errors[attribute]` holds it.
    pub message: String,
    /// `errors.full_messages`' entry for it.
    pub full: String,
    /// The error's symbol (`:taken`, `:blank`, …) when it has one; None for a custom message.
    pub kind: Option<&'static str>,
}

/// User#valid? for a new admin from the setup form, in 2.x's order: Devise's validatable
/// (email presence, uniqueness, format; password presence, length), then the model's own (email pattern and Google
/// alias, password complexity; D-NO-USER-NAME: no name). The preference inclusions always hold here.
/// `email` is already normalised.
pub fn validate(c: &Connection, locale: &str, email: &str, password: Option<&str>) -> Result<Vec<FieldError>, WebError> {
    let mut errors: Vec<(&str, Message)> = Vec::new();
    let blank = crate::v2::blank;
    let pattern = |pattern: &str, value: &str| crate::v2::validation_regex(pattern).map(|re| re.is_match(value)).map_err(|_| WebError::Config("invalid user validation pattern".into()));
    if blank(email) {
        errors.push(("email", Message::Kind("blank", None)));
    } else {
        if c.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE email = ?1)", [email], |r| r.get::<_, bool>(0))? {
            errors.push(("email", Message::Kind("taken", None)));
        }
        if !pattern(r"\A[^@\s]+@[^@\s]+\z", email)? {
            errors.push(("email", Message::Kind("invalid", None)));
        }
    }
    let password_blank = password.is_none_or(blank);
    if password_blank {
        errors.push(("password", Message::Kind("blank", None)));
    }
    if let Some(password) = password.filter(|p| !blank(p)) {
        let length = password.chars().count() as i64;
        if length < 8 { errors.push(("password", Message::Kind("too_short", Some(8)))); }
        if length > 128 { errors.push(("password", Message::Kind("too_long", Some(128)))); }
    }
    if !pattern(r"(?m)^[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}$", email)? {
        errors.push(("email", Message::Text(i18n::text(locale, "devise.registrations.new.email_invalid", &[]))));
    }
    if google_email_exists(c, email)? {
        errors.push(("email", Message::Kind("taken", None)));
    }
    if let Some(password) = password.filter(|p| !blank(p)) {
        if !super::user::password_complexity(password) {
            errors.push(("password", Message::Text(i18n::text(locale, "errors.messages.too_simple_password", &[]))));
        }
    }
    Ok(errors.into_iter().map(|(attribute, message)| {
        let (text, full) = full_message(locale, attribute, &message);
        let kind = match message { Message::Kind(kind, _) => Some(kind), Message::Text(_) => None };
        FieldError { attribute, message: text, full, kind }
    }).collect())
}

/// User::Email.google_email_username: the part before `@` and any `+tag`, downcased, for a Gmail address only.
pub(super) fn google_username(email: &str) -> Option<String> {
    if !(email.ends_with("@gmail.com") || email.ends_with("@googlemail.com")) {
        return None;
    }
    email.split('@').next().map(|local| local.split('+').next().unwrap_or(local).to_lowercase())
}

/// User::Email.google_email_exists? for a new account: some account already has the same Google username.
fn google_email_exists(c: &Connection, email: &str) -> Result<bool, WebError> {
    let Some(wanted) = google_username(email) else { return Ok(false) };
    let mut statement = c.prepare("SELECT email FROM users WHERE email LIKE '%@gmail.com' OR email LIKE '%@googlemail.com'")?;
    let stored = statement.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(stored.iter().any(|email| google_username(email).as_deref() == Some(wanted.as_str())))
}

/// POST /setup.
pub async fn create(State(app): State<App>, Extension(ctx): Extension<Ctx>, headers: HeaderMap) -> Result<Response, WebError> {
    if let Some(answer) = filters(&app, &ctx, &headers).await? {
        return Ok(answer);
    }
    let Some(fields) = submitted(&ctx.params) else {
        return Ok(super::layout::bad_request());
    };
    let email = fields.email.as_deref().map(normalized_email).unwrap_or_default();
    // Devise hashes on assignment, before validation, and only a present password.
    let hash = match fields.password.as_deref().filter(|p| !crate::v2::blank(p)) {
        Some(password) => match app.password_hash(password.to_string()).await? {
            None => return Ok(auth::busy()),
            Some(None) => return Err(WebError::Config("no random salt for the admin's password".into())),
            Some(Some(hash)) => Some(hash),
        },
        None => None,
    };
    let (locale, now, inner) = (ctx.locale, ctx.now, app.clone());
    let password = fields.password.clone();
    let outcome = {
        let email = email.clone();
        app.db(move |c| {
            let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
            // The check that makes this the only admin, in the transaction that inserts it.
            if admin_exists(&tx)? {
                return Ok(Created::AdminExists);
            }
            let errors = validate(&tx, locale, &email, password.as_deref())?;
            let Some(hash) = hash.filter(|_| errors.is_empty()) else { return Ok(Created::Invalid(errors)) };
            let at = format_time(now);
            tx.execute(
                "INSERT INTO users (email, encrypted_password, admin, confirmed_at, setup_completed, locale, time_zone, display_currency, \
                 otp_secret_key, created_at, updated_at) VALUES (?1, ?2, 1, ?3, 1, ?4, 'UTC', 'USD', ?5, ?3, ?3)",
                (&email, &hash, &at, locale, inner.cipher.encrypt_binary(&super::settings::two_fa::new_seed())),
            )?;
            let id = tx.last_insert_rowid();
            tx.commit()?;
            Ok(Created::Admin(id, hash))
        }).await?
    };
    match outcome {
        Created::AdminExists => Ok(layout::redirect(StatusCode::FOUND, &ctx.path("/"))),
        Created::Invalid(errors) => form(&app, &ctx, Form { status: StatusCode::UNPROCESSABLE_ENTITY, errors: &errors, email: &email, flash_now: Vec::new() }).await,
        Created::Admin(id, hash) => {
            let mut session = ctx.session.lock();
            session.setup_token = None;
            session.platform_identity = None;
            auth::sign_in(&mut session, id, &hash, true);
            drop(session);
            Ok(layout::redirect(StatusCode::FOUND, &ctx.path("/bots")))
        }
    }
}

enum Created {
    AdminExists,
    Invalid(Vec<FieldError>),
    Admin(i64, String),
}

/// One row of the language step.
struct LanguageRow {
    code: &'static str,
    name: &'static str,
    greeting: String,
    next: String,
}

#[derive(Template)]
#[template(path = "language/new.html")]
struct LanguageView<'a> {
    v: &'a Ctx,
    rows: Vec<LanguageRow>,
}

/// GET /language (D-LANGUAGE-STEP): the greeting, the 15 languages in the picker's order with the preselected one first
/// and checked, and Continue, which is `GET /setup?locale=<code>`. Preselected: `?locale=` when it names one of ours (the
/// way back from the admin form), else the browser's (`first_choice`). The page renders in that language. No token: it
/// shows nothing of the install; /setup applies the token gate. Closed, as /setup is, once an admin exists.
pub async fn language(State(app): State<App>, Extension(ctx): Extension<Ctx>, headers: HeaderMap) -> Result<Response, WebError> {
    if app.db(admin_exists).await? {
        return Ok(layout::redirect(StatusCode::FOUND, &ctx.path("/")));
    }
    let preselected = ctx.params.query("locale").and_then(locale::known).unwrap_or_else(|| first_choice(&headers).code());
    let ctx = Ctx { locale: preselected, ..ctx };
    let rows = std::iter::once(preselected).chain(locale::PICKER.into_iter().filter(|code| *code != preselected))
        .map(|code| LanguageRow { code, name: locale::native_name(code), greeting: i18n::text(code, "setup.language.greeting", &[]), next: i18n::text(code, "setup.continue", &[]) })
        .collect();
    let body = LanguageView { v: &ctx, rows }.render()?;
    onboarding_page(&ctx, StatusCode::OK, body, Vec::new(), "language")
}
