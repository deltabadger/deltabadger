//! The listed 3.0 differences (Rust only, plan 2026-10-23 tracker tabs and scope frames), applied to a page Rails
//! rendered so the grids compare it byte for byte with this crate's:
//! - the tracker's holdings card moves into the record as its first pane, Holdings / Positions / Transactions, unfolded
//!   (no "Show more"), each row carrying its class for the pane's All / Crypto / Stocks / Cash filter;
//! - the venue switch loads into the `tracker_scope` frame, which wraps the chart, the tiles and the record;
//! - the bot list's status filter loads into the `bots_scope` frame around the tiles, and the merge bar is always there.
//!
//! Every step finds its anchor or fails, so a change to Rails' markup is a failure here, never a silent pass.
use deltabadger::web::{i18n, tracker::record};

fn find(page: &str, anchor: &str, from: usize) -> Result<usize, String> {
    page[from..].find(anchor).map(|i| from + i).ok_or_else(|| format!("3.0 transform: anchor {anchor:?} not found"))
}

/// The page's locale, from `<html lang>`; a frame's answer has none, and its links carry the locale's prefix instead.
fn locale(page: &str) -> Result<&'static str, String> {
    let Some(at) = page.find("<html lang=\"").map(|at| at + 12) else {
        return Ok(deltabadger::web::locale::LOCALES.iter().copied().find(|l| page.contains(&format!("=\"/{l}/"))).unwrap_or("en"));
    };
    let lang = &page[at..at + page[at..].find('"').ok_or("3.0 transform: unterminated lang")?];
    deltabadger::web::locale::LOCALES.iter().copied().find(|l| *l == lang).ok_or_else(|| format!("3.0 transform: unknown locale {lang}"))
}

/// A row's class for the holdings filter, from its type pill: the plan's mapping.
fn class_of(row: &str) -> &'static str {
    let Some(at) = row.find("<span class=\"pill pill--quiet\">") else { return "" };
    let kind = &row[at + 31..at + 31 + row[at + 31..].find('<').unwrap_or(0)];
    match kind { "Crypto" => "crypto", "Stock" | "ETF" | "Fund" => "stocks", "Cash" | "Stable" => "cash", _ => "" }
}

/// Rails' holdings card, unfolded and its rows marked: the card this crate renders, and the classes its rows hold.
fn card(rails: &str) -> Result<(String, Vec<&'static str>), String> {
    let mut card = rails.to_string();
    if let Some(open) = card.find("          <details class=\"tracker-holdings__more\">\n") {
        let close = find(&card, "          </details>\n", open)?;
        let block = card[open..close].to_string();
        let first = find(&block, "</summary>\n", 0)? + 11;
        // The folded rows, at the shown rows' indent, at the end of the rows' list (whose `</div>` precedes the fold).
        let rows = block[first..].replace("\n              <div class=\"tracker-holdings__row\">", "\n            <div class=\"tracker-holdings__row\">");
        let rows = rows.strip_prefix("              ").map(|r| format!("            {r}")).ok_or("3.0 transform: folded rows' indent")?;
        card.replace_range(open..close + 21, "");
        let end = card[..open].rfind("        </div>\n").ok_or("3.0 transform: the rows' list")?;
        card.insert_str(end, &rows);
    }
    let mut classes = vec![];
    let marker = "<div class=\"tracker-holdings__row\">";
    let mut out = String::new();
    let mut rest = card.as_str();
    while let Some(at) = rest.find(marker) {
        let end = rest[at..].find("\n</div>").map_or(rest.len(), |e| at + e);
        let class = class_of(&rest[at..end]);
        if !class.is_empty() && !classes.contains(&class) { classes.push(class); }
        out.push_str(&rest[..at]);
        out.push_str(&format!("<div class=\"tracker-holdings__row\" data-order-filter-target=\"row\" data-order-type=\"all{}\">", if class.is_empty() { String::new() } else { format!(" {class}") }));
        rest = &rest[at + marker.len()..];
    }
    out.push_str(rest);
    Ok((out, classes))
}

/// The Holdings pane around `card` (`card` ends as the page placed it: with its blank line).
fn pane(locale: &str, card: &str, classes: &[&'static str]) -> Result<String, String> {
    let label = |key: &str| i18n::text(locale, key, &[]);
    let mut options = vec![record::option("all", label("tracker.record.all"), true)];
    for class in ["crypto", "stocks", "cash"] {
        if classes.contains(&class) { options.push(record::option(class, label(&format!("tracker.holdings_filter.{class}")), false)); }
    }
    let filter = if options.len() > 2 { format!("        {}\n", record::segmented(label("tracker.holdings_filter.label"), None, true, options).map_err(|_| "3.0 transform: the filter".to_string())?) } else { String::new() };
    Ok(format!("  <div class=\"tracker-record__pane\" data-pane=\"hold\"\n       data-controller=\"order-filter\" data-tracker--record-target=\"pane\">\n    <div class=\"tracker-record__filters filters\" data-action=\"{}\">\n{filter}    </div>\n    <div class=\"tracker-record__table\">\n{card}    </div>\n  </div>\n\n", record::FILTERS_ACTION))
}

/// The record with the Holdings pane first and on: its switch gains the option, Positions goes off.
fn record_with(record: &str, locale: &str, pane: &str) -> Result<String, String> {
    let mut out = record.to_string();
    let pos_on = "<button type=\"button\" class=\"segmented__option is-on\" role=\"radio\" aria-checked=\"true\" tabindex=\"0\" data-value=\"pos\"";
    let at = find(&out, pos_on, 0)?;
    let line = out[..at].rfind('\n').ok_or("3.0 transform: switch line")? + 1;
    let line_end = find(&out, "\n", at)? + 1;
    let pos_line = out[line..line_end].to_string();
    let hold_line = pos_line.replace("data-value=\"pos\"", "data-value=\"hold\"")
        .replace(&format!(">{}</button>", i18n::text(locale, "tracker.positions", &[])), &format!(">{}</button>", i18n::text(locale, "tracker.holdings", &[])));
    let pos_off = pos_line.replace("class=\"segmented__option is-on\" role=\"radio\" aria-checked=\"true\" tabindex=\"0\"", "class=\"segmented__option\" role=\"radio\" aria-checked=\"false\" tabindex=\"-1\"");
    out.replace_range(line..line_end, &format!("{hold_line}{pos_off}"));
    let tx = find(&out, "  <div class=\"tracker-record__pane tracker-record__pane--off\" data-pane=\"tx\"", 0)?;
    out.insert_str(tx, pane);
    let pos = "  <div class=\"tracker-record__pane\" data-pane=\"pos\"";
    let at = find(&out, pos, 0)?;
    out.replace_range(at..at + pos.len(), "  <div class=\"tracker-record__pane tracker-record__pane--off\" data-pane=\"pos\"");
    Ok(out)
}

const RECORD: &str = "    <div class=\"tracker-record\" data-controller=\"tracker--record\">";
const CARD: &str = "<div class=\"widget tracker-holdings";

/// The populated tracker index: holdings into the record, the scope frame, the venue switch and the bar's far end.
pub fn tracker_page(page: &str) -> Result<String, String> {
    let locale = locale(page)?;
    let mut out = page.to_string();
    // The holdings region: the card and its blank line, between the tiles and the record.
    let start = find(&out, CARD, 0)?;
    let record_at = find(&out, RECORD, start)?;
    let (card, classes) = card(&out[start..record_at])?;
    out.replace_range(start..record_at, "");
    let record_at = find(&out, RECORD, 0)?;
    let record_end = find(&out, "</form>  </div>\n</div>\n\n  </div>\n", record_at)? + 23;
    let record = record_with(&out[record_at..record_end], locale, &pane(locale, &card, &classes)?)?;
    out.replace_range(record_at..record_end + 1, &format!("{record}</turbo-frame>\n"));
    // The frame opens under the bar.
    let bar = find(&out, "<div class=\"tracker-exchanges\">", 0)?;
    let bar_end = ["\n    <div class=\"widget widget--chart\">", "\n    \n"].iter().filter_map(|m| out[bar..].find(m)).min().ok_or("3.0 transform: the bar's end")? + bar;
    out.insert_str(bar_end, "\n    <turbo-frame id=\"tracker_scope\" class=\"scope-frame\" target=\"_top\" data-turbo-action=\"advance\">");
    // The venue switch and the bar's far end.
    let region = out[bar..bar_end].to_string();
    let mut switched = region.replacen("data-action=\"click-&gt;segmented#toggle\">", "data-action=\"click-&gt;segmented#toggle\" data-segmented-frame=\"tracker_scope\">", if region.contains("class=\"segmented ") { 1 } else { 0 });
    switched = switched.lines().map(|line| if line.contains("<a class=\"segmented__option") && !line.contains("segmented__option--action") && !line.contains("data-broken") {
        line.replace(" data-segmented-target=\"option\"", " data-turbo-frame=\"tracker_scope\" data-segmented-target=\"option\"")
    } else { line.to_string() }).collect::<Vec<_>>().join("\n") + if region.ends_with('\n') { "\n" } else { "" };
    switched = switched.replacen("<div class=\"tracker-exchanges__end buttons-no-outline\">", "<div class=\"tracker-exchanges__end buttons-no-outline\" id=\"tracker_bar_end\">", 1);
    out.replace_range(bar..bar_end, &switched);
    Ok(out)
}

/// The first-sync page: its card into the record (one venue, so no switch and no frame).
pub fn first_sync_page(page: &str) -> Result<String, String> {
    let locale = locale(page)?;
    let mut out = page.to_string();
    let start = find(&out, CARD, 0)?;
    let record_at = find(&out, RECORD, start)?;
    let (card, classes) = card(&out[start..record_at])?;
    // The card sat at the record's indent, and its own line is gone with it.
    out.replace_range(start - 4..record_at, "");
    let record_at = find(&out, RECORD, 0)?;
    let record_end = find(&out, "</form>  </div>\n</div>\n", record_at)? + 23;
    let record = record_with(&out[record_at..record_end], locale, &pane(locale, &card, &classes)?)?;
    out.replace_range(record_at..record_end, &record);
    Ok(out)
}

/// Where the `<div` opened at `start` closes: the index just past its `</div>`.
fn div_end(page: &str, start: usize) -> Result<usize, String> {
    let mut depth = 0usize;
    let mut at = start;
    loop {
        let open = page[at..].find("<div").map(|i| at + i);
        let close = page[at..].find("</div>").map(|i| at + i).ok_or("3.0 transform: unclosed div")?;
        match open {
            Some(open) if open < close => { depth += 1; at = open + 4; }
            _ => { depth -= 1; at = close + 6; if depth == 0 { return Ok(at); } }
        }
    }
}

/// The bot list (a page holding its head): the status filter loads into `bots_scope` around the tiles, the head's
/// actions carry their id, and the merge bar is there whatever the listed bots allow. Any other page is returned as is.
pub fn bots_page(page: &str) -> Result<String, String> {
    let Some(head) = page.find("<div class=\"page-head page-head--bots\">") else { return Ok(page.to_string()) };
    let locale = locale(page)?;
    let mut out = page.to_string();
    let head_end = div_end(&out, head)?;
    let region = out[head..head_end].to_string()
        .replacen("data-action=\"click-&gt;segmented#toggle\">", "data-action=\"click-&gt;segmented#toggle\" data-segmented-frame=\"bots_scope\">", 1)
        .replace(" data-segmented-target=\"option\" data-action=\"segmented#select\" href=", " data-segmented-target=\"option\" data-action=\"segmented#select\" data-turbo-frame=\"bots_scope\" href=")
        .replacen("<div class=\"page-head__actions\">", "<div class=\"page-head__actions\" id=\"bots_actions\">", 1);
    // The path prefix the page's own links carry (a locale's, or none), read off its New button.
    let new = find(&region, "/bots/new\"", 0)?;
    let prefix = region[..new].rsplit("action=\"").next().ok_or("3.0 transform: the New button")?.to_string();
    out.replace_range(head..head_end, &region);
    let tiles = find(&out, "<div class=\"itiles\"", 0)?;
    let tiles_end = div_end(&out, tiles)?;
    let bar = if out.contains("<div class=\"merge-bar\"") { String::new() } else {
        let t = |key: &str| i18n::t(locale, key, &[]);
        format!("\n    <div class=\"merge-bar\" data-bots-merge-target=\"bar\" hidden>\n      <div class=\"merge-bar__assets\">\n        <span class=\"merge-bar__hint\" data-bots-merge-target=\"hint\">{}</span>\n        <span class=\"merge-bar__hint text-danger\" data-bots-merge-target=\"alone\" hidden></span>\n        <div class=\"ticker-group ticker-group--fold\" data-controller=\"ticker-fold\" data-bots-merge-target=\"stack\" hidden></div>\n        <span class=\"merge-bar__warning\" data-bots-merge-target=\"warning\" hidden></span>\n      </div>\n      <div class=\"merge-bar__actions\">\n        <button name=\"button\" type=\"button\" class=\"rbutton rbutton--secondary\" data-action=\"bots-merge#cancel\">{}</button>\n        <form data-turbo-frame=\"modal\" data-bots-merge-target=\"form\" action=\"{prefix}/bots/merge/new\" accept-charset=\"UTF-8\" method=\"get\">\n          <span data-bots-merge-target=\"ids\" hidden></span>\n          <button name=\"button\" type=\"submit\" class=\"rbutton rbutton--primary\" disabled=\"disabled\" data-bots-merge-target=\"confirm\">{}</button>\n</form>      </div>\n    </div>\n",
            t("bot.merge.pick_hint"), t("button.cancel"), t("button.merge"))
    };
    out.insert_str(tiles_end, &format!("\n</turbo-frame>{bar}"));
    out.insert_str(tiles, "<turbo-frame id=\"bots_scope\" class=\"scope-frame\" target=\"_top\" data-turbo-action=\"advance\">\n  ");
    Ok(out)
}
