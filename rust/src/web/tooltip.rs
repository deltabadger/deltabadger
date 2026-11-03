//! AssetsController#tooltip: the hover card of any `.ticker` pill (ticker_tooltips_controller.js fetches it).
use super::{auth, i18n::escape, layout::{self, Ctx}, App, WebError};
use axum::{extract::{Extension, State}, http::StatusCode, response::{IntoResponse, Response}};
use rusqlite::OptionalExtension;

struct Asset { symbol: String, name: String, color: Option<String>, image: Option<String>, category: Option<String> }

/// A present `asset_id` decides alone: digits name a row (one past the column's range names none, as ActiveRecord answers
/// nil), anything else is "not found", never the symbol's asset. Without one, the symbol, case-insensitive, best ranked.
pub async fn show(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    let id = ctx.params.query("asset_id").filter(|id| !crate::ruby::blank(id)).map(|id| {
        if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) { id.parse::<i64>().ok() } else { None }
    });
    // `params[:symbol].to_s.strip` then present?; String#upcase is Unicode, SQLite's UPPER is ASCII (as Rails' SQL is).
    let symbol = crate::ruby::strip(ctx.params.query("symbol").unwrap_or("")).to_uppercase();
    let read = |r: &rusqlite::Row| Ok(Asset { symbol: r.get::<_, Option<String>>(0)?.unwrap_or_default(), name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                                               color: r.get(2)?, image: r.get(3)?, category: r.get(4)? });
    let asset = app.db(move |c| {
        const COLUMNS: &str = "SELECT symbol, name, color, image_url, category FROM assets";
        Ok(match id {
            Some(Some(id)) => c.query_row(&format!("{COLUMNS} WHERE id = ?1"), [id], read).optional()?,
            Some(None) => None,
            None if crate::ruby::blank(&symbol) => None,
            None => c.query_row(&format!("{COLUMNS} WHERE UPPER(symbol) = ?1 ORDER BY market_cap_rank IS NULL, market_cap_rank, id LIMIT 1"), [symbol], read).optional()?,
        })
    }).await?;
    let Some(asset) = asset else { return Ok((StatusCode::NOT_FOUND, [(axum::http::header::CONTENT_TYPE, "text/html")]).into_response()) };
    Ok(layout::html(StatusCode::OK, card(&asset)?))
}

/// assets/_tooltip_card.html.erb.
fn card(a: &Asset) -> Result<String, WebError> {
    let bg = super::colors::ensure_contrast(a.color.as_deref().unwrap_or("#8A9BA8")).ok_or_else(|| WebError::Config("asset colour".into()))?;
    // asset_logo_url: an asset of a category that is not cash draws its image; otherwise its flag, else its image.
    let cash = a.category.as_deref().is_none_or(|c| crate::ruby::blank(c) || matches!(c, "Fiat" | "Currency"));
    let logo = if cash { super::tracker::row::fiat_flag(&a.symbol).or_else(|| a.image.clone()) } else { a.image.clone() };
    let mut out = String::from("<div class=\"tooltip--ticker__col\">\n");
    match logo.filter(|l| !crate::ruby::blank(l)) {
        Some(logo) => out += &format!("    <span class=\"tooltip--ticker__logo\">\n      <img class=\"tooltip--ticker__logo-img\" src=\"{}\" alt=\"\" loading=\"lazy\">\n    </span>\n", escape(&logo)),
        None => out += &format!("    <span class=\"tooltip--ticker__logo tooltip--ticker__logo--fallback\" style=\"background: {}\">{}</span>\n",
                                escape(&bg), escape(&a.symbol.chars().take(2).collect::<String>().to_uppercase())),
    }
    out += &format!("</div>\n<div class=\"tooltip--ticker__col\">\n  <div class=\"tooltip--ticker__header\">\n    <span class=\"tooltip--ticker__name\">{}</span>\n    <div>\n      <span class=\"tooltip--ticker__symbol\">{}</span>\n      &middot; \n",
                    escape(&a.name), escape(&a.symbol));
    // TICKER_TYPE_LABELS.
    let label = match a.category.as_deref() { Some("Cryptocurrency") => Some("Crypto"), Some("Stock" | "Common Stock") => Some("Stock"), Some("ETF") => Some("ETF"),
                                              Some("Fund") => Some("Fund"), Some("Fiat" | "Currency") => Some("Cash"), _ => None };
    if let Some(label) = label { out += &format!("        <span class=\"tooltip--ticker__info\">{label}</span>\n"); }
    out += "    </div>\n  </div>\n</div>\n";
    Ok(out)
}
