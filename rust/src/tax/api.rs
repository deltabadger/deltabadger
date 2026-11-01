//! BotApi::Tax::{Params, GenerateReport, ReportStatus, DownloadReport, Generating} for the REST routes and the three MCP
//! tools (GenerateTaxReportTool, GetTaxReportStatusTool, DownloadTaxReportTool). Crypto only, as Rails: no report scope.
use super::job;
use crate::web::bot_api::{Domain, Res};
use crate::web::mcp::protocol::tool_text;
use crate::web::{App, WebError};
use rusqlite::Connection;
use serde_json::{json, Value};

/// The routes and tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route { Create, Status, Download }

impl Route {
    pub fn tool(name: &str) -> Option<Route> {
        match name { "generate_tax_report" => Some(Route::Create), "get_tax_report_status" => Some(Route::Status), "download_tax_report" => Some(Route::Download), _ => None }
    }
}

/// The MCP tools' names (tools::NAMES).
pub const NAMES: [&str; 3] = ["generate_tax_report", "get_tax_report_status", "download_tax_report"];

/// What a route answers: the envelope, create's 202, or the report itself.
pub enum Out { Res(Res), Accepted(Res), Csv { csv: String, filename: String } }

fn failure(status: Domain, code: &'static str, message: impl Into<String>) -> Res { Res::failure(status, code, message.into()) }

/// `value.to_s` as Ruby writes a parameter (a JSON number as its literal, nil as nothing).
fn to_s(v: Option<&Value>) -> String { v.map(crate::web::mcp::tools::str_value).unwrap_or_default() }

/// The registry's name for a code (Tax::Jurisdictions.for), from the recorded jurisdictions.
fn jurisdiction(code: &str) -> Option<(String, String)> {
    crate::web::bot_api::tax_jurisdictions().data["jurisdictions"].as_array()?.iter()
        .find(|j| j["code"] == code).map(|j| (j["name"].as_str().unwrap_or("").to_string(), j["method"].as_str().unwrap_or("").to_string()))
}

/// BotApi::Tax::Params.resolve: (code, year, name, method).
fn params(country: Option<&Value>, year: Option<&Value>) -> Result<(String, i64, String, String), Res> {
    let code = to_s(country).trim().to_uppercase();
    if crate::ruby::blank(&code) || to_s(year).trim().is_empty() {
        return Err(failure(Domain::ValidationFailed, "missing_required_parameter", "Missing required parameter(s): country, year."));
    }
    let number = year.and_then(crate::web::bot_api::orders::integer).filter(|y| (2009..=2100).contains(y));
    let Some(number) = number else { return Err(failure(Domain::ValidationFailed, "invalid_year", "year must be a four-digit year.")) };
    let Some((name, method)) = jurisdiction(&code) else {
        return Err(failure(Domain::ValidationFailed, "unknown_country", format!("Unknown country code '{code}'. Use 'list_tax_jurisdictions' to see supported countries.")));
    };
    Ok((code, number, name, method))
}

/// BotApi::Boolean.parse(value, default:): only true, false, 'true' and 'false' answer; absent or empty is the default.
fn boolean(v: Option<&Value>, default: bool) -> Option<bool> {
    match v {
        None | Some(Value::Null) => Some(default),
        Some(Value::String(s)) if s.is_empty() => Some(default),
        Some(Value::Array(a)) if a.is_empty() => Some(default),
        Some(Value::Object(o)) if o.is_empty() => Some(default),
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::String(s)) if s == "true" => Some(true),
        Some(Value::String(s)) if s == "false" => Some(false),
        Some(_) => None,
    }
}

fn generating() -> Res {
    failure(Domain::Conflict, "report_generating", "A tax report is already being generated for this account. Check 'get_tax_report_status' and try again once it is ready.")
}

/// D-TAX-NOT-PORTED-GATE's answer for a jurisdiction named `name`.
pub fn not_ported_message(name: &str) -> String { format!("Tax reports for {name} are not served by this build yet.") }
/// The CoinGecko refusal (A17 lifts it).
pub const COINGECKO: &str = "Tax reports need Deltabadger market data on this build: historical prices from CoinGecko are not fetched yet.";

/// D-TAX-NOT-PORTED-GATE and the CoinGecko refusal, as the REST 501 and the tool's error text.
fn not_ported(message: String) -> Res { failure(Domain::NotPorted, "not_ported", message) }

/// BotApi::Tax::GenerateReport#call. In ONE write transaction under the files' lock (Rails' `with_lock` around the
/// check and the enqueue): generating → 409; the report present and not forced → 409; the report and its refusal step
/// aside; the request inserted and `pending_report` written; commit; the stale files removed. Any failure before the
/// commit puts both back.
pub fn create(c: &Connection, app: &App, user: i64, args: &serde_json::Map<String, Value>, locale: &str) -> Result<Res, WebError> {
    let (code, year, name, method) = match params(args.get("country"), args.get("year")) { Ok(p) => p, Err(res) => return Ok(res) };
    let Some(force) = boolean(args.get("force"), false) else { return Ok(failure(Domain::ValidationFailed, "invalid_flag", "force must be true or false.")) };
    let Some(stablecoin) = boolean(args.get("stablecoin_as_fiat"), false) else { return Ok(failure(Domain::ValidationFailed, "invalid_flag", "stablecoin_as_fiat must be true or false.")) };
    let (_, configured) = crate::web::bots::market_data(c, app)?;
    if !configured && method != "wealth_snapshot" {
        return Ok(failure(Domain::ValidationFailed, "market_data_not_configured", "Market data provider is not configured. Set up CoinGecko or Deltabadger market data in Settings first."));
    }
    if !super::jurisdictions::ported(&code, year) { return Ok(not_ported(not_ported_message(&name))); }
    if crate::web::tracker::report::coingecko(c, app)? {
        return Ok(not_ported(COINGECKO.into()));
    }
    let tmp = &app.config.tmp_dir;
    let path = job::report_path(tmp, user, &code, year, "crypto");
    let _files = job::files(user, "crypto");
    let tx = crate::engine::model::immediate(c)?;
    if job::generating(&tx, user)? { return Ok(generating()); }
    if path.exists() && !force {
        return Ok(failure(Domain::Conflict, "report_ready", format!("A tax report for {name} ({year}) is already available. \
                         Use 'download_tax_report' to retrieve it, or 'generate_tax_report' with force to replace it.")));
    }
    let request = job::token();
    let io = |e: std::io::Error| WebError::Config(e.to_string());
    let report = job::Aside::step(&path, &request).map_err(io)?;
    let refusal = match job::Aside::step(&job::refusal_path(tmp, user, &code, year, "crypto"), &request) {
        Ok(r) => r,
        Err(e) => { if let Some(a) = report { let _ = a.restore(); } return Err(io(e)); }
    };
    let now = app.now();
    let ask = job::Ask { user, scope: "crypto", country: &code, year, stablecoin_as_fiat: stablecoin, locale };
    let written = (|| -> Result<bool, WebError> {
        if !job::enqueue(&tx, &ask, &request, now)? { return Ok(false); }
        crate::web::tracker::report::set_pending(&tx, user, &crate::codec::format_time(now), json!({ "country": code, "year": year, "report_scope": "crypto" }))?;
        tx.commit()?;
        Ok(true)
    })();
    match written {
        Ok(true) => {
            for a in [report, refusal].into_iter().flatten() { a.drop_stale().map_err(io)?; }
            if let Ok(w) = app.job_wakers() { w.wake(job::JOB, None, None); }
            Ok(Res::success(json!({ "country": code, "year": year, "name": name, "ready": false, "state": "generating" })))
        }
        other => {
            for a in [report, refusal].into_iter().flatten() { a.restore().map_err(io)?; }
            other.map(|_| generating())
        }
    }
}

/// BotApi::Tax::ReportStatus: ready → refused → generating → none.
pub fn status(c: &Connection, app: &App, user: i64, args: &serde_json::Map<String, Value>) -> Result<Res, WebError> {
    let (code, year, _, _) = match params(args.get("country"), args.get("year")) { Ok(p) => p, Err(res) => return Ok(res) };
    let tmp = &app.config.tmp_dir;
    let _files = job::files(user, "crypto");
    let ready = job::report_path(tmp, user, &code, year, "crypto").exists();
    let refusal = job::refusal(tmp, user, &code, year, "crypto");
    let state = if ready { "ready" } else if refusal.is_some() { "refused" } else if job::generating(c, user)? { "generating" } else { "none" };
    let mut data = json!({ "country": code, "year": year, "ready": ready, "state": state });
    if let Some(r) = refusal { data["reason"] = r["reason"].clone(); data["symbols"] = r["symbols"].clone(); }
    Ok(Res::success(data))
}

/// BotApi::Tax::DownloadReport.refused_because.
pub fn refused_because(reason: &Value, symbols: &Value) -> String {
    let symbols = job::symbols(&json!({ "symbols": symbols }));
    if reason == "excess_return_of_capital" {
        format!("a return of capital on {symbols} exceeded its cost basis, and the report has no row for that gain. Deltabadger does not report it yet.")
    } else {
        format!("{symbols} are tokenized securities, whose tax treatment differs from crypto. Deltabadger does not report them yet.")
    }
}

/// BotApi::Tax::DownloadReport: the file first, then the refusal, then not found. Never deletes.
pub fn download(app: &App, user: i64, args: &serde_json::Map<String, Value>) -> Result<Result<(String, String), Res>, WebError> {
    let (code, year, _, _) = match params(args.get("country"), args.get("year")) { Ok(p) => p, Err(res) => return Ok(Err(res)) };
    let tmp = &app.config.tmp_dir;
    let _files = job::files(user, "crypto");
    match std::fs::read(job::report_path(tmp, user, &code, year, "crypto")) {
        Ok(bytes) => {
            let csv = String::from_utf8(bytes).map_err(|_| WebError::Config("a tax report that is not UTF-8".into()))?;
            return Ok(Ok((csv, format!("deltabadger-tax-report-{}-{year}.csv", code.to_lowercase()))));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(WebError::Config(e.to_string())),
    }
    if let Some(r) = job::refusal(tmp, user, &code, year, "crypto") {
        return Ok(Err(failure(Domain::ValidationFailed, "report_refused", format!("No report for {code} ({year}): {}", refused_because(&r["reason"], &r["symbols"])))));
    }
    Ok(Err(failure(Domain::NotFound, "report_not_found", format!("No report found for {code} ({year}). Use 'generate_tax_report' to create one first."))))
}

/// The REST actions (Api::V1::TaxReportsController).
pub fn rest(c: &Connection, app: &App, user: i64, route: Route, args: &serde_json::Map<String, Value>) -> Result<Out, WebError> {
    Ok(match route {
        Route::Create => { let res = create(c, app, user, args, crate::web::i18n::DEFAULT)?; if res.code.is_none() { Out::Accepted(res) } else { Out::Res(res) } }
        Route::Status => Out::Res(status(c, app, user, args)?),
        Route::Download => match download(app, user, args)? { Ok((csv, filename)) => Out::Csv { csv, filename }, Err(res) => Out::Res(res) },
    })
}

/// The tools' texts (`present` and `report_error`).
pub fn mcp(c: &Connection, app: &App, user: i64, name: &str, args: &Value) -> Result<Value, WebError> {
    let none = serde_json::Map::new();
    let args = args.as_object().unwrap_or(&none);
    let error = |res: &Res| tool_text(&res.message, true);
    Ok(match Route::tool(name) {
        Some(Route::Create) => {
            let res = create(c, app, user, args, crate::web::i18n::DEFAULT)?;
            if res.code.is_some() { return Ok(error(&res)); }
            tool_text(&format!("Tax report generation started for {} ({}). This runs in the background and may take a few minutes depending on transaction volume. \
                                Use 'get_tax_report_status' to check when it's ready.", crate::web::mcp::tools::str_value(&res.data["name"]), res.data["year"]), false)
        }
        Some(Route::Status) => {
            let res = status(c, app, user, args)?;
            if res.code.is_some() { return Ok(error(&res)); }
            let d = &res.data;
            let (country, year) = (crate::web::mcp::tools::str_value(&d["country"]), &d["year"]);
            tool_text(&match d["state"].as_str() {
                Some("ready") => format!("Report for {country} ({year}) is ready. Use 'download_tax_report' to retrieve it."),
                Some("generating") => "A report is being generated for this account. Check again in a minute.".into(),
                Some("refused") => format!("Report for {country} ({year}) cannot be generated: {}", refused_because(&d["reason"], &d["symbols"])),
                _ => format!("Report for {country} ({year}) is not ready yet or has not been generated. Use 'generate_tax_report' to start one."),
            }, false)
        }
        Some(Route::Download) => match download(app, user, args)? { Ok((csv, _)) => tool_text(&csv, false), Err(res) => error(&res) },
        None => return Err(WebError::Config("unregistered tax tool".into())),
    })
}
