use super::*;
use crate::figures::json::J;
use crate::web::{colors, i18n::{self, escape}, bots::{Segmented, SegmentedOption}};
use askama::Template;
use rusqlite::OptionalExtension;

fn modes(locale: &str) -> Result<String, FiguresError> {
    let label = |key: &str| i18n::text(locale, &format!("bot.details.stats.chart.{key}"), &[]);
    Segmented { fluid:true,label:label("modes"),key:Some("chart-mode"),links:false,
        options:vec![SegmentedOption{value:"pnl",label:label("pnl"),active:true,href:None},SegmentedOption{value:"value",label:label("value"),active:false,href:None}]
    }.render().map_err(|_| FiguresError::Data("Chart modes unavailable".into()))
}
fn toggles(locale: &str) -> String {
    let mut out = "<div class=\"widget--chart__orders\">\n".to_string();
    for key in ["orders","prices"] {
        let action = if key == "orders" { "toggleOrders" } else { "togglePrices" };
        out.push_str(&format!("<label class=\"toggle-inline\">\n<div class=\"toggle toggle--small\">\n<input type=\"checkbox\" data-bot--chart-target=\"{key}\" data-action=\"change->bot--chart#{action}\">\n<div class=\"toggle__style\"></div>\n</div>\n<span class=\"label\">{}</span>\n</label>\n",i18n::t(locale,&format!("bot.details.stats.chart.{key}"),&[])));
    }
    out.push_str("</div>\n");out
}
#[allow(clippy::too_many_arguments)]
pub(super) fn render(c: &Connection, s: &Subject, m: &Metrics, missing: &[String], hidden: bool, locale: &str, zone: &str) -> Result<String, FiguresError> {
    let charted:bool = c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id=?1)",[s.bot.id],|r|r.get(0))?;
    if !charted { return Ok("<div id=\"chart\" hidden=\"hidden\">\n</div>\n".into()); }
    if m.prices_stale || !missing.is_empty() || !m.chart_omitted.is_empty() {
        return Ok(format!("<div id=\"chart\" class=\"widget widget--chart\">\n{NO_VALUE}\n</div>\n"));
    }
    let Some(page) = chart::page(c,s,m,hidden)? else {
        let pnl = if hidden { "" } else { "<div class=\"widget--chart__pnl\">&nbsp;<small>&nbsp;</small></div>" };
        let percent = if hidden { "widget--chart__pnl" } else { "widget--chart__percent" };
        return Ok(format!("<div id=\"chart\" class=\"widget widget--chart\">\n<div>\n<div class=\"widget--chart__head\">\n<div class=\"widget--chart__summary\">\n<div class=\"widget--chart__date\">&nbsp;</div>\n{pnl}\n<div class=\"{percent}\">&nbsp;</div>\n</div>\n</div>\n<div class=\"widget--chart__plot\">\n{}\n</div>\n</div>\n</div>\n",include_str!("../../../templates/svg/_landscape_empty.html")));
    };
    let zone = crate::web::timezone::zone(zone).unwrap_or(chrono_tz::UTC);
    let mut attributes = "data-controller=\"bot--chart\" data-action=\"mouseover@document->bot--chart#focus mouseleave@document->bot--chart#blur click@document->bot--chart#select turbo:before-stream-render@document->bot--chart#restore\"".to_string();
    for (key,value) in page.attributes(&zone,3) {
        if key != "logo-assets" { attributes.push_str(&format!(" data-bot--chart-{key}-value=\"{}\"",escape(&value))); }
    }
    let mut logos = vec![];
    for (key,id) in &page.logo_assets {
        let logo = c.query_row("SELECT image_url,color FROM assets WHERE id=?1",[id],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?))).optional()?;
        if let Some((image,colour)) = logo {
            let colour = colors::ensure_contrast(colour.as_deref().unwrap_or(colors::NEUTRAL)).ok_or_else(|| FiguresError::Data("Invalid asset colour".into()))?;
            logos.push((key.clone(), J::Obj(vec![("image".into(),image.map_or(J::Null,J::Str)),("color".into(),J::Str(colour))])));
        }
    }
    attributes.push_str(&format!(" data-bot--chart-buy-logos-value=\"{}\"",escape(&J::Obj(logos).write())));
    let mode = if !hidden && m.chart.invested.iter().any(|n|n.to_f()>0.0) { modes(locale)? } else { String::new() };
    let money = if hidden { "" } else { "<div class=\"widget--chart__pnl\" data-bot--chart-target=\"pnl\">&nbsp;</div>" };
    let percent = if hidden { "widget--chart__pnl" } else { "widget--chart__percent" };
    Ok(format!("<div id=\"chart\" class=\"widget widget--chart\">\n<div {attributes}>\n<div class=\"widget--chart__head\">\n<div class=\"widget--chart__modes\" data-action=\"segmented:change->bot--chart#mode\">\n{mode}\n</div>\n<div class=\"widget--chart__summary\" data-bot--chart-target=\"summary\">\n<div class=\"widget--chart__date\" data-bot--chart-target=\"date\">&nbsp;</div>\n{money}\n<div class=\"{percent}\" data-bot--chart-target=\"percent\">&nbsp;</div>\n</div>\n</div>\n<div class=\"widget--chart__plot\">\n<canvas data-bot--chart-target=\"analyzerChart\" width=\"10\" height=\"2\"></canvas>\n<div class=\"widget--chart__buys\" data-bot--chart-target=\"buys\"></div>\n</div>\n<div class=\"widget--chart__axis\" data-bot--chart-target=\"axis\"></div>\n{}\n</div></div>\n",toggles(locale)))
}
