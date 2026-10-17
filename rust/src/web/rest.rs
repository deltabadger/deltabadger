//! The REST API, `/api/v1` (Api::V1::BaseController, an ActionController::API): bearer tokens with the `api` scope,
//! one tool gate per route, and every answer in the `{data, error}` envelope. Outside the session/CSRF pipeline, as
//! ActionController::API is outside ApplicationController.
//!
//! Every route Rails draws is matched here, with its `(.:format)` suffix. One this build does not serve yet still
//! authenticates and gates as Rails does, then answers the 501 `not_ported` envelope; a path Rails does not route
//! (a locale prefix included: the API is outside the locale scope) is the 404 every missing page is.
use super::bot_api::{self, Domain, Res};
use super::mcp::reads;
use super::{bearer, consent, headers, layout, App, Params, WebError, BODY_READ_TIMEOUT, FORM_LIMIT};
use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub fn router() -> Router<App> {
    Router::new().route("/api/v1", any(handle)).route("/api/v1/{*rest}", any(handle))
}

/// What a route's before_action gates on: one tool, or the tool its `type` parameter names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gate { Tool(&'static str), BotType, OrderType }

/// The routes this build serves; every other one is `Unported`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action { Bots, Exchanges, Transactions, TaxJurisdictions, Indices, Write(bot_api::Write), Read(Reading), Unported }

/// The reads that may ask a venue (BotApi::Bots::Get, Exchanges::Balances, Orders::ListOpen, Portfolio::Summary).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reading { Details, Balances, Orders, Portfolio }

/// `bad`: a path parameter that is no UTF-8 once decoded, which Rails' router refuses (400) before any controller.
struct Route { gate: Gate, action: Action, path: Vec<(&'static str, String)>, bad: bool }

/// Journey::Router::Utils.unescape_uri: each `%XX` is its byte, anything else (a `+`, a stray `%`) stays as it is; the
/// bytes must then be UTF-8.
fn unescape(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[i], bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(high), Some(low)) => { out.push((high * 16 + low) as u8); i += 3; }
            (b, ..) => { out.push(b); i += 1; }
        }
    }
    String::from_utf8(out).ok()
}

/// config/routes.rb's `namespace :api { namespace :v1 { ... } }`, matched on the path past `/api/v1` as Rails' router
/// matches it: a dynamic segment is anything without `/` or `.`, and the last segment may end in `.<format>`.
fn route(method: &Method, path: &str) -> Option<Route> {
    let rest = path.strip_prefix("/api/v1/")?;
    let mut segments: Vec<&str> = rest.split('/').collect();
    let mut format = None;
    if let Some((last, ext)) = segments.last().copied().and_then(|last| last.split_once('.')) {
        if ext.is_empty() || ext.contains('.') { return None; }
        *segments.last_mut()? = last;
        format = Some(ext);
    }
    if segments.iter().any(|s| s.is_empty() || s.contains('.')) { return None; }
    let get = method == Method::GET || method == Method::HEAD;
    let post = method == Method::POST;
    let delete = method == Method::DELETE;
    let update = method == Method::PATCH || method == Method::PUT;
    let tool = |name| Some(Gate::Tool(name));
    let mut path: Vec<(&'static str, String)> = vec![];
    let (gate, action) = match segments.as_slice() {
        ["bots"] if get => (tool("list_bots"), Action::Bots),
        ["bots"] if post => (Some(Gate::BotType), Action::Unported),
        ["bots", id, rest @ ..] => {
            path.push(("id", id.to_string()));
            let name = match rest {
                [] if get => Some("get_bot_details"),
                [] if update => Some("update_bot_settings"),
                [] if delete => Some("delete_bot"),
                ["start"] if post => Some("start_bot"),
                ["stop"] if post => Some("stop_bot"),
                ["archive"] if post => Some("archive_bot"),
                ["archive"] if delete => Some("unarchive_bot"),
                ["liquidations"] if post => Some("liquidate_exited_asset"),
                ["redeploy"] if post => Some("answer_redeploy_offer"),
                _ => None,
            };
            let action = if name == Some("get_bot_details") { Action::Read(Reading::Details) } else { name.and_then(bot_api::Write::tool).map_or(Action::Unported, Action::Write) };
            (name.map(Gate::Tool), action)
        }
        ["transactions"] if get => (tool("list_transactions"), Action::Transactions),
        ["transactions", "account"] if get => (tool("list_account_transactions"), Action::Unported),
        ["transactions", "export"] if get => (tool("export_transactions_csv"), Action::Unported),
        ["transactions", "account", id, "transfer_link"] if post => { path.push(("id", id.to_string())); (tool("set_transfer_link"), Action::Unported) }
        ["transactions", "account", id, "price"] if method == Method::PATCH => { path.push(("id", id.to_string())); (tool("set_transaction_price"), Action::Unported) }
        ["tracker", "sync"] if post => (tool("sync_tracker"), Action::Unported),
        ["exchanges"] if get => (tool("list_exchanges"), Action::Exchanges),
        ["exchanges", id, "balances"] if get => { path.push(("id", id.to_string())); (tool("get_exchange_balances"), Action::Read(Reading::Balances)) }
        ["orders"] if get => (tool("list_open_orders"), Action::Read(Reading::Orders)),
        ["orders"] if post => (Some(Gate::OrderType), Action::Unported),
        ["orders", id] if delete => { path.push(("id", id.to_string())); (tool("cancel_order"), Action::Unported) }
        ["portfolio"] if get => (tool("get_portfolio_summary"), Action::Read(Reading::Portfolio)),
        ["indices"] if get => (tool("list_indices"), Action::Indices),
        ["rules"] if get => (tool("list_rules"), Action::Unported),
        ["rules"] if post => (tool("create_rule"), Action::Unported),
        ["rules", id, rest @ ..] => {
            path.push(("id", id.to_string()));
            (match rest {
                [] if update => tool("update_rule_settings"),
                [] if delete => tool("delete_rule"),
                ["start"] if post => tool("start_rule"),
                ["stop"] if post => tool("stop_rule"),
                _ => None,
            }, Action::Unported)
        }
        ["tax", "jurisdictions"] if get => (tool("list_tax_jurisdictions"), Action::TaxJurisdictions),
        ["tax", "reports"] if post => (tool("generate_tax_report"), Action::Unported),
        ["tax", "reports", country, year, rest @ ..] if get => {
            path.extend([("country", country.to_string()), ("year", year.to_string())]);
            (match rest { [] => tool("get_tax_report_status"), ["download"] => tool("download_tax_report"), _ => None }, Action::Unported)
        }
        _ => (None, Action::Unported),
    };
    if let Some(format) = format { path.push(("format", format.to_string())); }
    let decoded: Option<Vec<_>> = path.iter().map(|(name, value)| unescape(value).map(|value| (*name, value))).collect();
    let bad = decoded.is_none();
    gate.map(|gate| Route { gate, action, path: decoded.unwrap_or(path), bad })
}

/// The request's parameters as Rails' `params` holds them (ActionDispatch::Http::Parameters#parameters): the body's,
/// then the query's over them, then the path's over both. A body Rails cannot parse is `Err`: its 400, raised where an
/// action first reads `params`. (ParamsWrapper's copy of the body under the controller's name is never read.)
struct Input<'a> { params: &'a Params, json: Option<Vec<u8>>, path: &'a [(&'static str, String)] }

impl Input<'_> {
    fn parse(&self) -> Result<Map<String, Value>, ()> {
        let mut out = match &self.json {
            // `content_length.zero?`: no parameters.
            Some(body) if body.is_empty() => Map::new(),
            // A body that is not an object is `_json` (ActionDispatch's JSON parser).
            Some(body) => match serde_json::from_slice::<Value>(body).map_err(|_| ())? { Value::Object(m) => m, other => Map::from_iter([("_json".to_string(), other)]) },
            None => object(super::bot::action_params::form(&self.params.form).map_err(|_| ())?),
        };
        out.extend(object(super::bot::action_params::form(&self.params.query).map_err(|_| ())?));
        out.extend(self.path.iter().map(|(name, value)| (name.to_string(), json!(value))));
        Ok(out)
    }
}

fn object(v: Value) -> Map<String, Value> { match v { Value::Object(m) => m, _ => Map::new() } }

/// The media types ActionDispatch parses as JSON (Mime[:json] and its synonyms).
fn json_body(content_type: Option<&str>) -> bool {
    let media = content_type.unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    matches!(media.as_str(), "application/json" | "text/x-json" | "application/jsonrequest")
}

/// An answer before the shared headers: the envelope a controller renders, or a response made below the controllers.
/// `Pending`: a read's venue fetch, run outside the database before the read is finished and rendered.
enum Answer { Json(StatusCode, String), Below(Box<Response>), Pending(Box<super::mcp::reads::Fetch>) }

/// Api::V1::BaseController#status_for.
fn status_for(status: Domain) -> StatusCode {
    match status {
        Domain::Success => StatusCode::OK, Domain::ValidationFailed => StatusCode::UNPROCESSABLE_ENTITY, Domain::NotFound => StatusCode::NOT_FOUND,
        Domain::PermissionDenied => StatusCode::FORBIDDEN, Domain::Conflict => StatusCode::CONFLICT, Domain::UpstreamFailed => StatusCode::BAD_GATEWAY,
        Domain::NotPorted => StatusCode::NOT_IMPLEMENTED, Domain::Raised => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn failure(status: StatusCode, code: &str, message: &str) -> Answer {
    Answer::Json(status, encode(&json!({"data": null, "error": {"code": code, "message": message}})))
}

/// render_result: the data on a success, the code and message on a failure. What Rails raises is its 500, below the
/// controllers.
fn render(res: Res) -> Answer {
    if res.status == Domain::Raised { return Answer::Below(Box::new(WebError::Config(res.message).into_response())); }
    match res.code {
        None => Answer::Json(status_for(res.status), encode(&json!({"data": res.data, "error": null}))),
        Some(code) => failure(status_for(res.status), code, &res.message),
    }
}

/// ApiOauthAuthentication: a read of the token (OauthBearerTokenResolver, no write of any token row), 403 for a token
/// without the `api` scope and 401 for every other refusal.
fn authenticate(c: &Connection, app: &App, authorization: Option<&str>) -> Result<Result<bearer::Bearer, Answer>, WebError> {
    Ok(bearer::resolve(c, authorization, "api", app.clock.as_ref())?.map_err(|why| {
        let status = if why == bearer::Refusal::InsufficientScope { StatusCode::FORBIDDEN } else { StatusCode::UNAUTHORIZED };
        failure(status, why.code(), why.message())
    }))
}

/// require_rest_tool!: the owner has the tool on now, and the client was granted it (ToolAccess#rest_enabled?).
fn require_tool(c: &Connection, who: bearer::Bearer, tool: &str) -> Result<Option<Answer>, WebError> {
    let (enabled, granted) = consent::rest_access(c, who.user_id, who.application_id)?;
    let allowed = enabled.iter().any(|n| n == tool) && granted.iter().any(|n| n == tool);
    Ok((!allowed).then(|| failure(StatusCode::FORBIDDEN, "tool_disabled", &format!("Tool '{tool}' is disabled for this user."))))
}

/// `params[:type]` as Ruby interpolates it.
fn type_text(v: Option<&Value>) -> String { v.map(super::mcp::tools::str_value).unwrap_or_default() }

/// BotsController#gate_create_by_type and OrdersController#gate_create_by_type: the tool a `type` names, or the 422.
fn typed_tool(gate: Gate, params: &Map<String, Value>) -> Result<&'static str, Answer> {
    let given = params.get("type");
    let text = type_text(given);
    if gate == Gate::BotType {
        // `(params[:type].presence || 'dca').to_s`
        let kind = if given.is_some_and(|v| !matches!(v, Value::Null | Value::Bool(false)) && !text.trim().is_empty()) { text.as_str() } else { "dca" };
        return match kind { "dca" => Ok("create_bot"), "index" => Ok("create_index_bot"), "signal" => Ok("create_signal_bot"),
            _ => Err(failure(StatusCode::UNPROCESSABLE_ENTITY, "invalid_bot_type", &format!("Unknown bot type '{text}'. Must be one of: dca, index, signal."))) };
    }
    ["market_buy", "market_sell", "limit_buy", "limit_sell"].into_iter().find(|t| *t == text)
        .ok_or_else(|| failure(StatusCode::UNPROCESSABLE_ENTITY, "invalid_order_type", &format!("Unknown order type '{text}'. Must be one of: market_buy, market_sell, limit_buy, limit_sell.")))
}

/// What BotsController hands the service: for an update, the settings it permits (`params.permit(*names)`: a scalar,
/// nil included, and `allocations` as it came); for a start, `params[:start_fresh]`.
fn service_arguments(write: bot_api::Write, params: &Map<String, Value>) -> Map<String, Value> {
    match write {
        bot_api::Write::Settings => params.iter()
            .filter(|(name, value)| super::bot::mcp_input::setting_names().contains(&name.as_str()) && (name.as_str() == "allocations" || !(value.is_array() || value.is_object())))
            .map(|(name, value)| (name.clone(), value.clone())).collect(),
        bot_api::Write::Lifecycle(super::bot::write::Action::Start) => params.get("start_fresh").map(|v| Map::from_iter([("start_fresh".to_string(), v.clone())])).unwrap_or_default(),
        bot_api::Write::Lifecycle(_) => Map::new(),
    }
}

fn dispatch(c: &Connection, app: &App, route: &Route, input: &Input, authorization: Option<&str>, method: &Method, path: &str) -> Result<Answer, WebError> {
    let who = match authenticate(c, app, authorization)? { Ok(who) => who, Err(answer) => return Ok(answer) };
    let parsed = std::cell::OnceCell::new();
    let params = || parsed.get_or_init(|| input.parse());
    let bad_request = || Answer::Below(Box::new(layout::bad_request()));
    let tool = match route.gate {
        Gate::Tool(tool) => tool,
        gate => {
            let Ok(params) = params() else { return Ok(bad_request()) };
            match typed_tool(gate, params) { Ok(tool) => tool, Err(answer) => return Ok(answer) }
        }
    };
    if let Some(refused) = require_tool(c, who, tool)? { return Ok(refused); }
    let user = who.user_id;
    Ok(match route.action {
        Action::Exchanges => render(bot_api::exchanges(c, user)?),
        Action::TaxJurisdictions => render(bot_api::tax_jurisdictions()),
        Action::Bots => { let Ok(p) = params() else { return Ok(bad_request()) }; render(bot_api::bots(c, user, p.get("status"))?) }
        Action::Transactions => {
            let Ok(p) = params() else { return Ok(bad_request()) };
            match bot_api::transactions(c, user, p.get("bot_id").unwrap_or(&Value::Null), p.get("limit").unwrap_or(&Value::Null))? {
                Some(res) => render(res),
                None => return Err(WebError::Config("a transactions parameter Rails cannot convert".into())),
            }
        }
        Action::Indices => { let Ok(p) = params() else { return Ok(bad_request()) }; render(bot_api::indices(c, app, p.get("exchange_name").unwrap_or(&Value::Null))?) }
        Action::Write(write) => {
            let Ok(p) = params() else { return Ok(bad_request()) };
            // `params[:id].to_i`: the path's id (it wins over the body's and the query's), its leading digits.
            let id = p.get("id").and_then(bot_api::to_i).unwrap_or(0);
            let submitted = super::bot::action_params::ActionParams::from_service(service_arguments(write, p));
            render(bot_api::write_result(bot_api::bot_write(c, app, user, id, write, &submitted)?))
        }
        Action::Read(reading) => {
            // The summary reads no parameter, so a body Rails cannot parse never reaches it.
            let none = Map::new();
            let p = if reading == Reading::Portfolio { &none } else { match params() { Ok(p) => p, Err(()) => return Ok(bad_request()) } };
            let ask = match reading {
                // `params[:id].to_i`
                Reading::Details => reads::Ask::Details(p.get("id").and_then(bot_api::to_i).unwrap_or(0)),
                Reading::Balances => reads::Ask::Balances { name: String::new(), id: Some(p.get("id").map(super::mcp::tools::str_value).unwrap_or_default()) },
                // `params[:exchange_name]`, looked up when present.
                Reading::Orders => reads::Ask::Orders(p.get("exchange_name").filter(|v| bot_api::present(v)).map(super::mcp::tools::str_value)),
                Reading::Portfolio => reads::Ask::Summary,
            };
            match reads::plan_ask(c, app, user, ask)? {
                reads::Planned::Done(read) => render(read.res),
                reads::Planned::Fetch(fetch) => Answer::Pending(Box::new(fetch)),
            }
        }
        Action::Unported => failure(StatusCode::NOT_IMPLEMENTED, "not_ported", &format!("{method} {path} is not served by this build yet.")),
    })
}

pub async fn handle(State(app): State<App>, request: Request) -> Response {
    let Some(params) = request.extensions().get::<Arc<Params>>().cloned() else {
        return WebError::Config("a request reached the routes without passing web::router's entry".into()).into_response();
    };
    let head = request.method() == Method::HEAD;
    let conditional = matches!(*request.method(), Method::GET | Method::HEAD)
        .then(|| super::header_text(request.headers(), "if-none-match").map(str::to_string)).flatten();
    let method = request.method().clone();
    let found = if params.path_locale.is_some() { None } else { route(&method, &params.route_path) };
    // ActionController::BadRequest from the router (a path parameter that is no UTF-8), answered by ShowExceptions
    // above every other middleware: public/400.html and its content type alone.
    if found.as_ref().is_some_and(|r| r.bad) {
        let body = super::assets::find("/400.html").map_or(&b"Bad Request"[..], |file| file.body);
        return (StatusCode::BAD_REQUEST, [(header::CONTENT_TYPE, "text/html; charset=UTF-8")], if head { &b""[..] } else { body }).into_response();
    }
    let answer = match found {
        None => Answer::Below(Box::new(layout::missing())),
        Some(route) => {
            let (parts, body) = request.into_parts();
            let authorization = super::header_text(&parts.headers, "authorization").map(str::to_string);
            let json = if json_body(super::header_text(&parts.headers, "content-type")) {
                match tokio::time::timeout(BODY_READ_TIMEOUT, to_bytes(body, FORM_LIMIT)).await {
                    Err(_) => return (StatusCode::REQUEST_TIMEOUT, [(header::CONNECTION, "close")], "The form did not arrive in time\n").into_response(),
                    Ok(Err(_)) => return (StatusCode::PAYLOAD_TOO_LARGE, "Form too large\n").into_response(),
                    Ok(Ok(bytes)) => Some(bytes.to_vec()),
                }
            } else { None };
            let app2 = app.clone();
            let shown = params.full_path.clone();
            let method = parts.method.clone();
            let answer = match app.db(move |c| dispatch(c, &app2, &route, &Input { params: &params, json, path: &route.path }, authorization.as_deref(), &method, &shown)).await {
                Ok(answer) => answer,
                Err(error) => Answer::Below(Box::new(error.into_response())),
            };
            // A venue read: fetched outside the database, then finished on one snapshot of it.
            match answer {
                Answer::Pending(fetch) => {
                    let fetched = reads::fetch(&app, *fetch).await;
                    match app.db(move |c| reads::finish_read(c, fetched)).await {
                        Ok(read) => render(read.res),
                        Err(error) => Answer::Below(Box::new(error.into_response())),
                    }
                }
                answer => answer,
            }
        }
    };
    finish(&app, answer, head, conditional.as_deref())
}

/// What the middleware below the router adds: the policy header, the controller's default headers (or, below the
/// controllers, the cache header alone), and Rack::ETag's weak tag on a 200 or 201 with a body. Rack::ConditionalGet
/// then answers a GET or HEAD whose If-None-Match is exactly that tag with a 304: no body, no content type. HEAD keeps
/// the headers and drops the body (Rack::Head).
fn finish(app: &App, answer: Answer, head: bool, if_none_match: Option<&str>) -> Response {
    let mut response = match answer {
        Answer::Json(status, body) => {
            let tag = (matches!(status, StatusCode::OK | StatusCode::CREATED) && !body.is_empty())
                .then(|| format!("W/\"{}\"", &hex::encode(Sha256::digest(body.as_bytes()))[..32]));
            let mut response = (status, [(header::CONTENT_TYPE, "application/json; charset=utf-8")], body).into_response();
            let fresh = status == StatusCode::OK && tag.is_some() && tag.as_deref() == if_none_match;
            if let Some(tag) = tag.and_then(|t| HeaderValue::from_str(&t).ok()) { response.headers_mut().insert(header::ETAG, tag); }
            headers::controller_defaults(response.headers_mut(), status, false);
            if fresh {
                *response.status_mut() = StatusCode::NOT_MODIFIED;
                response.headers_mut().remove(header::CONTENT_TYPE);
                *response.body_mut() = Body::empty();
            }
            response
        }
        Answer::Pending(_) => WebError::Config("a venue read was not fetched".into()).into_response(),
        Answer::Below(response) => {
            let mut response = *response;
            let status = response.status();
            headers::cache_control(response.headers_mut(), status, false);
            response.extensions_mut().insert(headers::BelowControllers);
            response
        }
    };
    // ActionDispatch::ContentSecurityPolicy::Middleware sends no policy with a 304.
    if response.status() != StatusCode::NOT_MODIFIED { headers::policy(response.headers_mut(), &headers::new_nonce(), app.config.force_ssl); }
    if head { *response.body_mut() = Body::empty(); }
    response
}

/// `render json:` (Oj.optimize_rails, `escape_html_entities_in_json` on), in one pass: a string with Oj's escapes, a
/// Float as Oj's Rails encoder prints it, an Integer as its digits. The data's decimals and times are already their
/// rendered text (`bot_api`).
pub fn encode(value: &Value) -> String {
    fn write(value: &Value, out: &mut String) {
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => match n.as_f64().filter(|_| n.is_f64()) {
                Some(f) => out.push_str(&crate::sync::wire::stored_float(f)),
                None => out.push_str(&n.to_string()),
            },
            Value::String(s) => crate::sync::wire::stored_string(s, out),
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() { if i > 0 { out.push(','); } write(item, out); }
                out.push(']');
            }
            Value::Object(members) => {
                out.push('{');
                for (i, (key, item)) in members.iter().enumerate() {
                    if i > 0 { out.push(','); }
                    crate::sync::wire::stored_string(key, out);
                    out.push(':');
                    write(item, out);
                }
                out.push('}');
            }
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// script/rust/record_api_json.rb: `{data: value, error: nil}.to_json` for every kind of value an answer holds.
    #[test]
    fn encode_writes_what_rails_renders() {
        let fixture: Value = serde_json::from_str(include_str!("../../tests/fixtures/api_json_vectors.json")).unwrap();
        let mut checked = 0;
        for vector in fixture["vectors"].as_array().unwrap() {
            let (kind, input, rendered) = (vector[0].as_str().unwrap(), vector[1].as_str().unwrap(), vector[2].as_str().unwrap());
            let decimal = |text: &str| json!(crate::ruby::BigDec::parse(text).unwrap().to_s_f());
            let value = match kind {
                "float" => json!(f64::from_bits(u64::from_str_radix(input, 16).unwrap())),
                "integer" => input.parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::from(input.parse::<u64>().unwrap())),
                // BigDecimal("-0") renders "-0.0"; a decimal column never reads back a negative zero.
                "decimal" if input == "-0" => continue,
                "decimal" => decimal(input),
                "string" => json!(input),
                "time" => json!(crate::ruby::iso8601_ms(input.parse().unwrap())),
                "nil" => Value::Null,
                "true" => json!(true),
                "false" => json!(false),
                "object" => json!({"b": 1, "a": [null, "x", 2.5], "c": {"z": decimal("1.10")}}),
                other => panic!("unknown vector kind {other}"),
            };
            assert_eq!(encode(&json!({"data": value, "error": null})), rendered, "{kind} {input}");
            checked += 1;
        }
        assert_eq!(checked, 676, "a vector was dropped or added");
    }

    #[test]
    fn routes_take_rails_format_suffix_and_segments() {
        let found = |m: Method, p: &str| route(&m, p).map(|r| (r.gate, r.action, r.path));
        assert_eq!(found(Method::GET, "/api/v1/bots.json").map(|r| r.1), Some(Action::Bots));
        assert_eq!(found(Method::HEAD, "/api/v1/tax/jurisdictions").map(|r| r.1), Some(Action::TaxJurisdictions));
        assert!(found(Method::GET, "/api/v1/bots.json.x").is_none());
        assert!(found(Method::GET, "/api/v1/bots/1.2.3").is_none());
        assert!(found(Method::DELETE, "/api/v1/bots").is_none());
        assert!(found(Method::PUT, "/api/v1/transactions/account/1/price").is_none());
        assert_eq!(found(Method::PUT, "/api/v1/bots/7.xml").map(|r| r.2), Some(vec![("id", "7".into()), ("format", "xml".into())]));
        assert_eq!(found(Method::POST, "/api/v1/orders").map(|r| r.0), Some(Gate::OrderType));
        // The router hands the action decoded parameters: `%30` is a digit, `+` and a stray `%` stay as they are.
        assert_eq!(found(Method::POST, "/api/v1/bots/1%30/stop").map(|r| r.2), Some(vec![("id", "10".into())]));
        assert_eq!(found(Method::POST, "/api/v1/bots/1+%/stop").map(|r| r.2), Some(vec![("id", "1+%".into())]));
        assert!(route(&Method::POST, "/api/v1/bots/%E2%82/stop").is_some_and(|r| r.bad));
    }
}
