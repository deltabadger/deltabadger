//! The authorization endpoint: `GET /oauth/authorize` shows the consent screen to the signed-in
//! owner, `POST` approves and `DELETE` denies. Doorkeeper's AuthorizationsController with the app's
//! view (app/views/doorkeeper/authorizations/new.html.erb) and its after-authorization hook
//! (ConnectedClients::RecordConsent), on the Rust session: these three go through the pipeline like
//! any page, so approving and denying need the session's CSRF token.
//!
//! A request is never redirected to a URI the client did not register. Until `client_id` and
//! `redirect_uri` have been checked against the client's row, every refusal is shown here, as a page
//! (or, on a POST, as JSON); only after that may an error travel to the redirect URI.
use super::auth::{self, User};
use super::layout::{self, Ctx};
use super::oauth::{self, present, redirect_uri_allowed, redirect_with, scopes, scopes_valid, text, Application, Uri, TOOL_DEFAULTS, TOOL_GROUPS};
use super::{headers, i18n, i18n::Arg, App, WebError};
use crate::codec::format_time;
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

/// The two surfaces a grant covers: (scope, form field, `users` column, `connected_clients` column, on by default).
struct Surface {
    scope: &'static str,
    field: &'static str,
    settings: &'static str,
    column: &'static str,
    /// MCP tools start as the catalogue says; REST tools all start off.
    defaults: bool,
}
const SURFACES: [Surface; 2] = [
    Surface { scope: "mcp", field: "granted_mcp_groups", settings: "mcp_settings", column: "mcp_tools", defaults: true },
    Surface { scope: "api", field: "granted_rest_groups", settings: "rest_settings", column: "rest_tools", defaults: false },
];

/// User#enabled_mcp_tool_names / #enabled_rest_tool_names: the defaults with the owner's overrides
/// (`<settings>['tool_permissions']`) laid over them, in the catalogue's order.
fn enabled_tools(c: &Connection, user_id: i64, surface: &Surface) -> Result<Vec<String>, WebError> {
    let stored: Option<String> = c.query_row(&format!("SELECT {} FROM users WHERE id = ?1", surface.settings), [user_id], |r| r.get(0)).optional()?.flatten();
    let settings: Value = stored.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null);
    let overrides = settings["tool_permissions"].as_object();
    let on = |value: &Value| !matches!(value, Value::Null | Value::Bool(false));
    let mut enabled: Vec<String> = TOOL_DEFAULTS.iter()
        .filter(|(name, default)| overrides.and_then(|o| o.get(*name)).map_or(*default && surface.defaults, on))
        .map(|(name, _)| name.to_string()).collect();
    // Hash#merge: an override for a name the catalogue does not have comes after it.
    enabled.extend(overrides.into_iter().flatten().filter(|(name, value)| on(value) && !TOOL_DEFAULTS.iter().any(|(known, _)| known == name)).map(|(name, _)| name.clone()));
    Ok(enabled)
}

/// ConnectedClient#granted_mcp_tools / #granted_rest_tools: what the client was granted, without
/// names the catalogue no longer has. `None` when the client has no grant from this user yet.
fn granted_tools(c: &Connection, user_id: i64, application_id: i64, surface: &Surface) -> Result<Option<Vec<String>>, WebError> {
    let stored: Option<String> = c.query_row(&format!("SELECT {} FROM connected_clients WHERE user_id = ?1 AND oauth_application_id = ?2", surface.column),
                                             [user_id, application_id], |r| r.get(0)).optional()?;
    Ok(stored.map(|text| {
        let names: Vec<String> = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default().iter()
            .map(|name| name.as_str().map_or_else(|| name.to_string(), str::to_string)).collect();
        let mut known: Vec<String> = Vec::new();
        for name in names {
            if TOOL_DEFAULTS.iter().any(|(tool, _)| *tool == name) && !known.contains(&name) { known.push(name); }
        }
        known
    }))
}

/// The OAuth parameters of an authorization request, from the query (GET) or the consent form.
struct Asked {
    client_id: Option<String>,
    redirect_uri: Option<String>,
    response_type: Option<String>,
    response_mode: Option<String>,
    scope: Option<String>,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
}

impl Asked {
    fn of(ctx: &Ctx) -> Self {
        // Rails' `params`: the query wins over the form.
        let get = |name: &str| ctx.params.query(name).or_else(|| ctx.params.form(name)).map(str::to_string);
        Self { client_id: get("client_id"), redirect_uri: get("redirect_uri"), response_type: get("response_type"), response_mode: get("response_mode"),
               scope: get("scope"), state: get("state"), code_challenge: get("code_challenge"), code_challenge_method: get("code_challenge_method") }
    }

    /// ErrorResponse's `response_on_fragment?`: where an answer to the client goes.
    fn on_fragment(&self) -> bool {
        match present(self.response_mode.as_deref()) {
            Some(mode) => mode == "fragment",
            None => self.response_type.as_deref() == Some("token"),
        }
    }
}

/// Why an authorization request is refused: Doorkeeper's error name and its description.
struct Refusal {
    error: &'static str,
    description: String,
    /// The client's redirect URI, once it has been verified against the client's row. Only an error
    /// found after that carries it, and only such an error may be sent there.
    redirect_uri: Option<String>,
}

fn refusal(error: &'static str, description: &str) -> Refusal {
    Refusal { error, description: description.to_string(), redirect_uri: None }
}

/// A request that passed every check of PreAuthorization.
struct PreAuth {
    client: Application,
    redirect_uri: String,
    /// The scope string as asked, or the default: what the consent form carries.
    scope: String,
}

/// The first three checks of Doorkeeper's PreAuthorization: the client and its redirect URI. An
/// error here is never redirected.
fn client_checked(c: &Connection, asked: &Asked) -> Result<Result<(Application, String), Refusal>, WebError> {
    let Some(client_id) = present(asked.client_id.as_deref()) else { return Ok(Err(refusal("invalid_request", &text::missing("client_id")))) };
    let Some(client) = Application::by_uid(c, client_id)? else { return Ok(Err(refusal("invalid_client", text::INVALID_CLIENT))) };
    match present(asked.redirect_uri.as_deref()).filter(|uri| redirect_uri_allowed(uri, &client.redirect_uri)) {
        Some(redirect_uri) => { let redirect_uri = redirect_uri.to_string(); Ok(Ok((client, redirect_uri))) }
        None => Ok(Err(refusal("invalid_redirect_uri", text::INVALID_REDIRECT_URI))),
    }
}

/// Doorkeeper's PreAuthorization, checks in its order.
fn pre_authorize(c: &Connection, asked: &Asked) -> Result<Result<PreAuth, Refusal>, WebError> {
    let (client, redirect_uri) = match client_checked(c, asked)? {
        Ok(checked) => checked,
        Err(refused) => return Ok(Err(refused)),
    };
    let refused = |error, description: &str| Ok(Err(Refusal { redirect_uri: Some(redirect_uri.clone()), ..refusal(error, description) }));
    // The per-user REST token's application takes part in no OAuth flow.
    if client.personal { return refused("unauthorized_client", text::UNAUTHORIZED_CLIENT); }
    let Some(response_type) = present(asked.response_type.as_deref()) else { return refused("invalid_request", &text::missing("response_type")) };
    if response_type != "code" { return refused("unsupported_response_type", text::UNSUPPORTED_RESPONSE_TYPE); }
    if present(asked.response_mode.as_deref()).is_some_and(|mode| !["query", "fragment"].contains(&mode)) {
        return refused("unsupported_response_mode", text::UNSUPPORTED_RESPONSE_MODE);
    }
    // No scope asked: the default scope, if the client has it.
    let scope = match present(asked.scope.as_deref()) {
        Some(scope) => scope.to_string(),
        None => client.allowed_scopes().into_iter().filter(|scope| *scope == oauth::DEFAULT_SCOPE).collect::<Vec<_>>().join(" "),
    };
    if !scopes_valid(&scope, &client.allowed_scopes()) { return refused("invalid_scope", text::INVALID_SCOPE); }
    if present(asked.code_challenge.as_deref()).is_none() { return refused("invalid_request", text::CODE_CHALLENGE_REQUIRED); }
    // Rails also takes `plain`. A challenge that is its own verifier protects nothing, the metadata
    // names S256 only, and a client that is already connected never comes back here to refresh.
    if asked.code_challenge_method.as_deref() != Some("S256") { return refused("invalid_code_challenge_method", text::CODE_CHALLENGE_METHOD); }
    Ok(Ok(PreAuth { client, redirect_uri, scope }))
}

/// Rack's `Cache-Control` for the status. Oauth::BaseController has no `set_no_cache`, so these
/// pages are not `no-store` even for a signed-in owner.
fn cached_as_rails(mut response: Response) -> Response {
    let status = response.status();
    headers::cache_control(response.headers_mut(), status, false);
    response
}

#[derive(Template)]
#[template(path = "oauth/error.html")]
struct ErrorPage<'a> {
    description: &'a str,
}

/// Doorkeeper's stock error page, without a layout.
fn error_page(status: StatusCode, description: &str) -> Result<Response, WebError> {
    Ok(cached_as_rails(layout::html(status, ErrorPage { description }.render()?)))
}

fn refusal_status(refused: &Refusal) -> StatusCode {
    if refused.error == "invalid_client" { StatusCode::UNAUTHORIZED } else { StatusCode::BAD_REQUEST }
}

/// An answer for the client, sent through the owner's browser to the verified redirect URI.
fn to_client(asked: &Asked, redirect_uri: &str, parameters: &[(&str, &str)]) -> Response {
    cached_as_rails(layout::redirect(StatusCode::FOUND, &redirect_with(redirect_uri, parameters, asked.on_fragment())))
}

struct Group {
    name: &'static str,
    label: String,
    checked: bool,
}

struct Section {
    field: &'static str,
    label: String,
    groups: Vec<Group>,
}

#[derive(Template)]
#[template(path = "oauth/consent.html")]
struct ConsentPage<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    logo: String,
    title: String,
    description: String,
    warning: String,
    scopes_label: String,
    scope_lines: Vec<String>,
    sections: Vec<Section>,
    redirect_label: String,
    redirect_host: String,
    cancel: String,
    connect: String,
    asked: &'a Asked,
    pre_auth: &'a PreAuth,
}

/// The sections of the consent form: for each scope asked, the tool groups, each ticked or not.
/// First consent: only `read`. Again: a group only when the grant already covers every tool of it
/// the owner has on, so that approving without touching the form never widens the grant.
fn sections(c: &Connection, user: &User, pre_auth: &PreAuth) -> Result<Vec<Section>, WebError> {
    let asked = scopes(&pre_auth.scope);
    let mut out = Vec::new();
    for surface in SURFACES.iter().filter(|surface| asked.contains(&surface.scope)) {
        let granted = granted_tools(c, user.id, pre_auth.client.id, surface)?;
        let enabled = enabled_tools(c, user.id, surface)?;
        let groups = TOOL_GROUPS.iter().map(|(name, tools)| {
            let checked = match &granted {
                Some(granted) => {
                    let available: Vec<&String> = enabled.iter().filter(|tool| tools.contains(&tool.as_str())).collect();
                    !available.is_empty() && available.iter().all(|tool| granted.contains(tool))
                }
                None => *name == "read",
            };
            Group { name, label: i18n::t(i18n::DEFAULT, &format!("settings.mcp.tools_{name}"), &[]), checked }
        }).collect();
        out.push(Section { field: surface.field, label: i18n::t(i18n::DEFAULT, &format!("settings.mcp.authorize_groups_label_{}", surface.scope), &[]), groups });
    }
    Ok(out)
}

/// GET /oauth/authorize. Signed out: to the login page, which comes back here. The page is in
/// English whatever the account's language: Oauth::BaseController sets no locale.
pub async fn new(State(app): State<App>, Extension(ctx): Extension<Ctx>, headers: HeaderMap) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let asked = Asked::of(&ctx);
    let (found, asked) = app.db(move |c| {
        let found = match pre_authorize(c, &asked)? {
            Ok(pre_auth) => { let sections = sections(c, &user, &pre_auth)?; Ok((pre_auth, sections)) }
            Err(refused) => Err(refused),
        };
        Ok((found, asked))
    }).await?;
    let (pre_auth, sections) = match found {
        Ok(found) => found,
        Err(refused) => return error_page(refusal_status(&refused), &refused.description),
    };
    let t = |key: &str| i18n::t(i18n::DEFAULT, key, &[]);
    let csrf = ctx.csrf_token();
    let page = ConsentPage {
        v: &ctx, csrf: &csrf,
        logo: format!("{}/logo_email_app.png", app.config.request_origin(&headers).unwrap_or_default()),
        title: t("settings.mcp.authorize_title"),
        description: i18n::t(i18n::DEFAULT, "settings.mcp.authorize_description_html", &[("client_name", Arg::Text(&pre_auth.client.name))]),
        warning: t("settings.mcp.authorize_unverified"),
        scopes_label: t("settings.mcp.authorize_scopes_label"),
        // `t("settings.mcp.scope_<scope>", default: scope)`; both scopes have a text.
        scope_lines: scopes(&pre_auth.scope).iter().map(|scope| t(&format!("settings.mcp.scope_{scope}"))).collect(),
        sections,
        redirect_label: t("settings.mcp.authorize_redirect_label"),
        redirect_host: Uri::parse(&pre_auth.redirect_uri).and_then(|uri| uri.host).filter(|host| !host.is_empty()).unwrap_or_else(|| pre_auth.redirect_uri.clone()),
        cancel: t("button.cancel"), connect: t("button.connect"),
        asked: &asked, pre_auth: &pre_auth,
    };
    Ok(cached_as_rails(layout::html(StatusCode::OK, page.render()?)))
}

/// The values of the fields called `boxes` or `field`.
fn named<'a>(fields: &'a [(String, String)], boxes: &str, field: &str) -> Vec<&'a str> {
    fields.iter().filter(|(name, _)| name == boxes || name == field).map(|(_, value)| value.as_str()).collect()
}

/// ConnectedClients::RecordConsent: the client's grant becomes the tools of the groups left ticked,
/// as far as the owner has them on now. A surface whose scope the authorization does not carry is
/// left as it was.
fn record_consent(c: &Connection, ctx: &Ctx, user_id: i64, application_id: i64, granted_scopes: &[&str], now: &str) -> Result<(), WebError> {
    let mut tools: Vec<String> = Vec::new();
    for surface in &SURFACES {
        let names = if granted_scopes.contains(&surface.scope) {
            // `Array(params[field])`: the `field[]` boxes, or a single `field`; the query's when the
            // query names the field at all (Rails' `params`), else the form's.
            let boxes = format!("{}[]", surface.field);
            let mut ticked = named(&ctx.params.query, &boxes, surface.field);
            if ticked.is_empty() { ticked = named(&ctx.params.form, &boxes, surface.field); }
            let requested: Vec<&str> = TOOL_GROUPS.iter().filter(|(group, _)| ticked.contains(group)).flat_map(|(_, tools)| tools.iter().copied()).collect();
            enabled_tools(c, user_id, surface)?.into_iter().filter(|tool| requested.contains(&tool.as_str())).collect()
        } else {
            granted_tools(c, user_id, application_id, surface)?.unwrap_or_default()
        };
        tools.push(json!(names).to_string());
    }
    let stored: Option<(String, String)> = c.query_row("SELECT mcp_tools, rest_tools FROM connected_clients WHERE user_id = ?1 AND oauth_application_id = ?2",
                                                       [user_id, application_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let Some(stored) = stored else {
        c.execute("INSERT INTO connected_clients (user_id, oauth_application_id, mcp_tools, rest_tools, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                  (user_id, application_id, &tools[0], &tools[1], now))?;
        return Ok(());
    };
    // ActiveRecord writes, and moves `updated_at`, only when a list changed.
    let same = |a: &str, b: &str| serde_json::from_str::<Value>(a).ok() == serde_json::from_str::<Value>(b).ok();
    if !same(&stored.0, &tools[0]) || !same(&stored.1, &tools[1]) {
        c.execute("UPDATE connected_clients SET mcp_tools = ?1, rest_tools = ?2, updated_at = ?3 WHERE user_id = ?4 AND oauth_application_id = ?5",
                  (&tools[0], &tools[1], now, user_id, application_id))?;
    }
    Ok(())
}

/// POST /oauth/authorize: the owner approved. The code is written, then the grant of tools, then the
/// browser goes to the client's redirect URI with the code and the client's `state`.
pub async fn create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user_id) = ctx.user().map(|user| user.id) else { return Ok(auth::unauthenticated(&ctx)) };
    let asked = Asked::of(&ctx);
    let (clock, inner) = (app.clone(), ctx.clone());
    let (outcome, asked) = app.db(move |c| oauth::transaction(c, &*clock.clock, |c, now| {
        let now = format_time(now);
        let pre_auth = match pre_authorize(c, &asked)? {
            Ok(pre_auth) => pre_auth,
            Err(refused) => return Ok((Err(refused), asked)),
        };
        let granted = scopes(&pre_auth.scope);
        let code = oauth::new_token();
        c.execute("INSERT INTO oauth_access_grants (application_id, resource_owner_id, token, expires_in, redirect_uri, scopes, code_challenge, code_challenge_method, created_at) \
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                  (pre_auth.client.id, user_id, &code, oauth::CODE_SECONDS, &pre_auth.redirect_uri, granted.join(" "), &asked.code_challenge, &asked.code_challenge_method, &now))?;
        record_consent(c, &inner, user_id, pre_auth.client.id, &granted, &now)?;
        Ok((Ok((pre_auth.redirect_uri, code)), asked))
    })).await?;
    let state = asked.state.as_deref().unwrap_or("");
    Ok(match outcome {
        Ok((redirect_uri, code)) => to_client(&asked, &redirect_uri, &[("code", &code), ("state", state)]),
        // The client and its redirect URI were verified: the error is the client's to hear.
        Err(Refusal { error, description, redirect_uri: Some(redirect_uri) }) => {
            to_client(&asked, &redirect_uri, &[("error", error), ("error_description", &description), ("state", state)])
        }
        Err(refused) => {
            let mut body = json!({ "error": refused.error, "error_description": refused.description });
            if !state.trim().is_empty() { body["state"] = json!(state); }
            cached_as_rails((refusal_status(&refused), [(header::CONTENT_TYPE, "application/json; charset=utf-8")], body.to_string()).into_response())
        }
    })
}

/// DELETE /oauth/authorize: the owner said no. The client is told `access_denied`, at its verified
/// redirect URI; with an unknown client or redirect URI nobody is told anything but the owner.
pub async fn destroy(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)); }
    let asked = Asked::of(&ctx);
    let (checked, asked) = app.db(move |c| Ok((client_checked(c, &asked)?, asked))).await?;
    let redirect_uri = match checked {
        Ok((_, redirect_uri)) => redirect_uri,
        Err(refused) => return error_page(refusal_status(&refused), &refused.description),
    };
    // Doorkeeper finds no flow to deny for another response type, and renders this with a 200.
    if asked.response_type.as_deref() != Some("code") {
        return error_page(StatusCode::OK, text::UNSUPPORTED_GRANT_TYPE);
    }
    Ok(to_client(&asked, &redirect_uri, &[("error", "access_denied"), ("error_description", text::ACCESS_DENIED), ("state", asked.state.as_deref().unwrap_or(""))]))
}
