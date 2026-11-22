//! D-WIZARD-SHEET, D-WIZARD-NO-SENTENCE and D-WIZARD-TRACK (named divergences from the 2.x recordings): a DCA wizard step
//! is the sheet Settings uses, its track in the side column and the step in the pane, where 2.x draws it in the full-page
//! dialog with a progress bar and the conversational sentence on top. The step itself (the `.bot-creation-layout` subtree:
//! table, basket, search, form) is compared as it was, with 2.x's progress bar and sentence left out, 2.x's basket read as
//! open, and this crate's pane search box and Next button taken out — each required to be exactly the form 2.x's sentence
//! had for it. Around the step, this crate's lines must be exactly the sheet (`wizard/sheet.html`, or for the key step's 422
//! stream `wizard/_sheet_frame.html`) with one track, and 2.x's exactly the full-page shell. The track's three items are
//! checked against 2.x's progress bar (which is current) and its sentence (each item's text, and the request its slot
//! made: method, URL, fields, frame, direction). Then this crate's shell is put back as 2.x's around this crate's step,
//! so everything else is compared as before. On the chooser only the DCA tile's frame target and its dropped
//! `data-wizard-dir` are left out. The index and signal wizards, and a lazy page of results (no shell), are not touched.
//!
//! D-INDEX-IN-ASSETS and D-CHOOSER-SKIPPED: the asset step's search row holds the Asset / Index switch where 2.x's zoom icon
//! is (and at the hundred-asset cap, where 2.x draws no row, the switch alone); the chooser has no Index tile, and a user
//! without advanced bots is sent straight to the exchange step where 2.x drew the chooser.
use super::{attr, html, is_space, subtree_end};
use serde_json::{json, Value};

const LOCALES: [&str; 15] = ["en", "de", "fr", "it", "es", "pt", "nl", "pl", "ru", "bg", "el", "sv", "da", "cs", "sk"];
/// The track's items (exchange 0, assets 1, currency 2) in each order, and the item each of 2.x's four progress steps is.
const EXCHANGE_FIRST: ([usize; 3], [usize; 4]) = ([0, 1, 2], [0, 0, 1, 2]);
const ASSET_FIRST: ([usize; 3], [usize; 4]) = ([1, 0, 2], [1, 0, 0, 2]);
const ITEM_KEYS: [&str; 3] = ["exchange", "assets", "spendable"];

/// The path without its locale and query.
fn route(path: &str) -> &str {
    let path = path.split('?').next().unwrap_or_default();
    match path.get(..4) {
        Some(prefix) if prefix.starts_with('/') && prefix.ends_with('/') && prefix[1..3].bytes().all(|b| b.is_ascii_lowercase()) => &path[3..],
        _ => path,
    }
}

fn dca_step(path: &str) -> bool {
    let route = route(path);
    route.starts_with("/bots/dca_single_assets/") || route.starts_with("/bots/dca_multi_assets/")
}

fn text(line: &Value) -> &str { line.as_str().unwrap_or_default() }
fn opens(line: &Value, tag: &str) -> bool { let l = text(line).trim_start(); l.starts_with(&format!("<{tag} ")) || l == format!("<{tag}>") }
fn has_class(line: &Value, tag: &str, class: &str) -> bool { opens(line, tag) && attr(text(line), "class").is_some_and(|c| c.split(' ').any(|c| c == class)) }
fn depth(line: &Value) -> usize { let l = text(line); (l.len() - l.trim_start().len()) / 2 }
/// A text node's text (html::normalize writes it quoted).
fn words(line: &Value) -> Option<String> {
    let l = text(line).trim();
    l.starts_with('"').then(|| serde_json::from_str::<String>(l).unwrap_or_else(|_| l.trim_matches('"').to_string()).trim().to_string()).filter(|t| !t.is_empty())
}

/// `lines` moved to start at `to`'s depth.
fn reindent(lines: &[Value], to: usize) -> Vec<Value> {
    let from = lines.first().map_or(0, depth);
    lines.iter().map(|l| json!(format!("{}{}", "  ".repeat(to), &text(l)[from * 2..]))).collect()
}

/// The innermost element around `at` that `is` matches: its first line.
fn enclosing(lines: &[Value], at: usize, is: impl Fn(&Value) -> bool) -> Option<usize> {
    (0..at).rev().find(|&i| is(&lines[i]) && subtree_end(lines, i) > at)
}

fn positions(lines: &[Value], from: usize, to: usize, is: impl Fn(&Value) -> bool) -> Vec<usize> {
    (from..to).filter(|&i| is(&lines[i])).collect()
}

/// The subtrees of `lines[from..to]` that are children of the element at depth `parent`.
fn children(lines: &[Value], from: usize, to: usize, parent: usize) -> Vec<usize> {
    positions(lines, from, to, |l| depth(l) == parent + 1 && text(l).trim_start().starts_with('<') && !text(l).trim_start().starts_with("</"))
}

fn escape(value: &str) -> String { value.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;") }

/// The children of `<body>` of a normalized fragment, at depth 0.
fn body_of(lines: &[String]) -> Vec<Value> {
    let lines: Vec<Value> = lines.iter().map(|l| json!(l)).collect();
    let body = lines.iter().position(|l| text(l).trim() == "<body>").expect("a parsed document has a body");
    let end = subtree_end(&lines, body);
    reindent(&lines[body + 1..end - 1], 0)
}

/// What the sheet's frame holds, the track and the step as placeholders.
fn sheet_frame() -> String {
    include_str!("../../templates/wizard/_sheet_frame.html").replace("{{ track|safe }}", "<x-track></x-track>").replace("{{ content|safe }}", "<x-layout></x-layout>")
}

/// `wizard/sheet.html`'s modal frame, the track and the step as placeholders.
fn sheet_shell(close_url: &str, close_label: &str) -> Vec<Value> {
    let page = include_str!("../../templates/wizard/sheet.html").replace("{{ close_url }}", &escape(close_url)).replace("{{ close_label }}", &escape(close_label))
        .replace("{% include \"svg/_24x24_close.html\" %}", include_str!("../../templates/svg/_24x24_close.html"))
        .replace("{% include \"wizard/_sheet_frame.html\" %}", &sheet_frame());
    body_of(&html::normalize(&page))
}

/// The full-page dialog 2.x draws a step in (the `v2_full_page.html` fixture, as 2.x writes it), the step as
/// a placeholder.
fn full_page_shell(close_url: &str) -> Vec<Value> {
    let page = include_str!("v2_full_page.html").replace("{{ close_url }}", &escape(close_url))
        .replace("{% include \"svg/_24x24_close.html\" %}", include_str!("../../templates/svg/_24x24_close.html"))
        .replace("{{ content|safe }}", "<x-layout></x-layout>").replacen(" complete=\"\"", "", 1);
    body_of(&html::normalize(&page))
}

fn without_space(lines: &[Value]) -> Vec<Value> { lines.iter().filter(|l| !is_space(l)).cloned().collect() }

/// The request a link or a form at `at` makes: method, URL, its hidden fields (the CSRF token aside), its text field (name
/// and placeholder: a search), the frame it targets and the transition's direction.
#[derive(Debug, PartialEq)]
struct Request { method: String, url: String, fields: Vec<(String, String)>, search: Option<(String, String)>, frame: Option<String>, dir: Option<String> }

fn request(lines: &[Value], at: usize) -> Request {
    let line = text(&lines[at]);
    let (frame, dir) = (attr(line, "data-turbo-frame"), attr(line, "data-wizard-dir"));
    if opens(&lines[at], "a") { return Request { method: "get".into(), url: attr(line, "href").unwrap_or_default(), fields: vec![], search: None, frame, dir } }
    let inputs = positions(lines, at, subtree_end(lines, at), |l| opens(l, "input"));
    let mut fields: Vec<(String, String)> = inputs.iter().map(|i| text(&lines[*i]))
        .filter(|l| attr(l, "type").as_deref() == Some("hidden") && attr(l, "name").as_deref() != Some("authenticity_token"))
        .map(|l| (attr(l, "name").unwrap_or_default(), attr(l, "value").unwrap_or_default())).collect();
    fields.sort();
    let search = inputs.iter().map(|i| text(&lines[*i])).find(|l| attr(l, "type").as_deref() == Some("text"))
        .map(|l| (attr(l, "name").unwrap_or_default(), attr(l, "placeholder").unwrap_or_default()));
    Request { method: attr(line, "method").unwrap_or_default().to_lowercase(), url: attr(line, "action").unwrap_or_default(), fields, search, frame, dir }
}

/// What 2.x's sentence offered: per item (exchange, assets, currency) the request its slot made and the text it showed (the
/// venue's name, the tickers), and its search (the exchange's or the quote's active slot). None: the step has no sentence.
struct Slots { requests: [Option<Request>; 3], shown: [Option<Vec<String>>; 3], search: Option<Request>, advances: Vec<Request> }

fn slots(lines: &[Value], from: usize, to: usize) -> Option<(usize, Slots)> {
    let at = positions(lines, from, to, |l| has_class(l, "div", "conversational")).first().copied()?;
    let end = subtree_end(lines, at);
    let mut slots = Slots { requests: [None, None, None], shown: [None, None, None], search: None, advances: vec![] };
    let forms = positions(lines, at, end, |l| opens(l, "form"));
    slots.advances = forms.iter().map(|f| request(lines, *f)).filter(|r| r.url.ends_with("/pick_buyable_asset/advance") && r.fields.is_empty()).collect();
    let assets = positions(lines, at, end, |l| has_class(l, "div", "conversational__assets")).first().copied();
    let Some(assets) = assets else {
        // invite_only: the order switch is all the sentence holds, on the assets' side.
        slots.requests[1] = forms.first().map(|f| request(lines, *f));
        return Some((at, slots));
    };
    let assets_end = subtree_end(lines, assets);
    slots.requests[1] = positions(lines, assets, assets_end, |l| opens(l, "a") || opens(l, "form")).first().map(|a| request(lines, *a));
    let tickers: Vec<String> = positions(lines, assets, assets_end, |l| opens(l, "div") && attr(text(l), "data-ticker-symbol").is_some())
        .iter().map(|t| attr(text(&lines[*t]), "data-ticker-symbol").unwrap_or_default()).chain(
            positions(lines, assets, assets_end, |l| has_class(l, "span", "conversational__stack-count")).iter().filter_map(|c| words(&lines[c + 1]))).collect();
    if !tickers.is_empty() { slots.shown[1] = Some(tickers) }
    for child in children(lines, at, end, depth(&lines[at])) {
        let line = &lines[child];
        if has_class(line, "form", "conversational__link") || has_class(line, "span", "conversational__link") {
            if !opens(line, "form") { continue }
            let found = request(lines, child);
            if has_class(line, "form", "active") { slots.search = Some(found); continue }
            let name = positions(lines, child, subtree_end(lines, child), |l| opens(l, "input") && attr(text(l), "readonly").is_some()).first()
                .and_then(|i| attr(text(&lines[*i]), "value")).filter(|v| !v.is_empty());
            if let Some(name) = name { slots.shown[0] = Some(vec![name]) }
            slots.requests[0] = Some(found);
        } else if has_class(line, "form", "ticker") {
            let found = request(lines, child);
            if has_class(line, "form", "active") { slots.search = Some(found) } else { slots.requests[2] = Some(found) }
        }
    }
    Some((at, slots))
}

/// 2.x's progress bar in its step: the title and the share, 25 to 100.
fn progress(lines: &[Value], from: usize, to: usize) -> Result<(usize, String, usize), String> {
    let bar = positions(lines, from, to, |l| has_class(l, "div", "process-progress")).first().copied().ok_or("2.x's step has no progress bar")?;
    let end = subtree_end(lines, bar);
    let title = (bar..end).find(|i| opens(&lines[*i], "h4")).and_then(|h| words(&lines[h + 1])).ok_or("no title")?;
    let share = (bar..end).find(|i| has_class(&lines[*i], "div", "slider__style__progress")).and_then(|i| attr(text(&lines[i]), "style"))
        .and_then(|style| style.strip_prefix("width: ")?.strip_suffix('%')?.parse().ok()).ok_or("no share")?;
    Ok((bar, title, share))
}

/// The orders and locales the step may be in, by 2.x's progress bar (its title names the step, its share the place):
/// (locale, the items in their order, the current item's place).
/// A reading of 2.x's progress bar: the locale, the items in their order, the current item's place.
type Reading = (&'static str, [usize; 3], usize);

fn readings(title: &str, share: usize, flow: Option<&str>) -> Result<Vec<Reading>, String> {
    let place = (share / 25).checked_sub(1).filter(|place| *place < 4 && share.is_multiple_of(25)).ok_or(format!("a share of {share}%"))?;
    let mut out = vec![];
    for locale in LOCALES {
        for (key, step) in [("pick_exchange", 0), ("api", 0), ("currencies", 1), ("assets", 1), ("spendable", 2)] {
            if deltabadger::web::i18n::text(locale, &format!("bot.setup.progress_steps.{key}"), &[]) != title { continue }
            for (order, (items, steps)) in [("exchange_first", EXCHANGE_FIRST), ("asset_first", ASSET_FIRST)] {
                if steps[place] != step || flow.is_some_and(|flow| flow != order) { continue }
                out.push((locale, items, items.iter().position(|item| *item == step).unwrap()));
            }
        }
    }
    if out.is_empty() { return Err(format!("no DCA step reads \"{title}\" at {share}%")) }
    Ok(out)
}

/// This crate's track, item by item: its state, its text and its request.
fn items(lines: &[Value], at: usize) -> Vec<Drawn> {
    let end = subtree_end(lines, at);
    children(lines, at, end, depth(&lines[at])).into_iter().filter(|li| opens(&lines[*li], "li")).map(|li| {
        let li_end = subtree_end(lines, li);
        let class = attr(text(&lines[li]), "class").unwrap_or_default();
        let state = if class.contains("sheet__step--done") { "done" } else if class.contains("sheet__step--current") { "current" } else { "upcoming" };
        let state = if (state == "current") != (attr(text(&lines[li]), "aria-current").as_deref() == Some("step")) { "aria-current-mismatch" } else { state };
        let shown = (li..li_end).filter_map(|i| words(&lines[i])).collect();
        let control = positions(lines, li, li_end, |l| opens(l, "a") || opens(l, "form")).first().map(|c| request(lines, *c));
        (state.to_string(), shown, control)
    }).collect()
}

/// `lines[from..to]` with the subtrees at `holes` (in order) each as `<x-NAME>`.
fn with_holes(lines: &[Value], from: usize, to: usize, holes: &[(usize, &str)]) -> Vec<Value> {
    let mut out = vec![];
    let mut at = from;
    for (hole, name) in holes {
        out.extend_from_slice(&lines[at..*hole]);
        let indent = "  ".repeat(depth(&lines[*hole]));
        out.extend([json!(format!("{indent}<x-{name}>")), json!(format!("{indent}</x-{name}>"))]);
        at = subtree_end(lines, *hole);
    }
    out.extend_from_slice(&lines[at..to]);
    reindent(&out, 0)
}

/// Removes the subtree at `at` with the whitespace before it.
/// D-EXCHANGE-NO-SEARCH: a request filtering the venues (`?query=` not blank), which 2.x's exchange step answered with the
/// matches alone and this crate's, having no search, with every venue.
fn searched(path: &str) -> bool {
    path.split_once('?').is_some_and(|(_, q)| q.split('&').any(|kv| kv.strip_prefix("query=").is_some_and(|v| !v.trim_matches('+').is_empty())))
}

/// D-ASSETS-SEARCH-FIRST: the asset step's search (`.modal--search`) is its first child here, drawn first; 2.x's sits right
/// before the results (or the max-reached note). Moved back there, whitespace and all.
fn resettle_search(lines: &mut Vec<Value>) -> Result<(), String> {
    let Some(step) = positions(lines, 0, lines.len(), |l| has_class(l, "div", "wizard-assets")).first().copied() else { return Ok(()) };
    let kids = children(lines, step, subtree_end(lines, step), depth(&lines[step]));
    let Some(search) = kids.iter().copied().find(|k| has_class(&lines[*k], "div", "modal--search")) else { return Ok(()) };
    if kids.first() != Some(&search) { return Err("the asset step's search is not its first child".into()) }
    // Before the results or the cap's note; with neither, last in the step.
    let target = kids.iter().copied().find(|k| (opens(&lines[*k], "turbo-frame") && attr(text(&lines[*k]), "id").as_deref() == Some("asset-picker")) || has_class(&lines[*k], "p", "text-inactive"));
    let end = subtree_end(lines, step);
    let target = target.unwrap_or(if text(&lines[end - 1]).trim_start().starts_with("</") { end - 1 } else { end });
    let start = if is_space(&lines[search - 1]) { search - 1 } else { search };
    let moved: Vec<Value> = lines.drain(start..subtree_end(lines, search)).collect();
    let target = target - moved.len();
    let at = if is_space(&lines[target - 1]) { target - 1 } else { target };
    lines.splice(at..at, moved);
    Ok(())
}

/// D-WIZARD-SEARCH-STYLES: the asset step's search box in the wizard's own classes (`wizard-search`, its form and input), read
/// back as 2.x's (`modal--search`, the form's inline position, `modal--search__input`).
fn unrestyle(lines: &mut [Value]) {
    for line in lines.iter_mut() {
        let l = text(line);
        let back = if l.contains("<div class=\"wizard-search\">") { l.replace("<div class=\"wizard-search\">", "<div class=\"modal--search\">") }
            else if l.contains(" class=\"wizard-search__form\"") && l.trim_start().starts_with("<form") { l.replace(" class=\"wizard-search__form\"", "").replacen('>', " style=\"position:relative\">", 1) }
            else if l.contains("class=\"wizard-search__input\"") { l.replace("class=\"wizard-search__input\"", "class=\"modal--search__input\"") }
            else { continue };
        *line = json!(back);
    }
}

/// D-KEY-LOGO-BACK: the key card's venue logo and name are one link back to the same wizard's venue step
/// (`…/pick_exchange/new` beside the step's `…/add_api_key`, the step's locale prefix), read back as 2.x's header: the link
/// taken off, the logo and the name under the header itself.
fn unback(lines: &mut Vec<Value>, path: &str) -> Result<(), String> {
    let Some(at) = positions(lines, 0, lines.len(), |l| has_class(l, "a", "set-api__back")).first().copied() else { return Ok(()) };
    let want = path.split_once("/add_api_key").map(|(bot, _)| format!("{bot}/pick_exchange/new")).ok_or("a key card's Back off the key step")?;
    let line = text(&lines[at]).to_string();
    if attr(&line, "href").as_deref() != Some(want.as_str()) || attr(&line, "data-wizard-dir").as_deref() != Some("back") { return Err(format!("the venue is not Back to {want}: {line}")) }
    let end = subtree_end(lines, at);
    let mut inner: Vec<Value> = reindent(&lines[at + 1..end - 1], depth(&lines[at]));
    // The link's own whitespace inside it goes with it: the header keeps the space it had around the link.
    while inner.first().is_some_and(is_space) { inner.remove(0); }
    while inner.last().is_some_and(is_space) { inner.pop(); }
    lines.splice(at..end, inner);
    Ok(())
}

fn cut(lines: &mut Vec<Value>, at: usize) {
    let end = subtree_end(lines, at);
    let start = if at > 0 && is_space(&lines[at - 1]) { at - 1 } else { at };
    lines.drain(start..end);
}

/// One item as 2.x's step implies it: its state, its text (None: not compared) and its request.
type Expected<'a> = (String, Option<Vec<String>>, Option<&'a Request>);
/// One item of this crate's track: its state, its text and its request.
type Drawn = (String, Vec<String>, Option<Request>);

/// The track's checks against 2.x's step: one reading of its progress bar (locale, order, current item) under which every
/// item has its state by position, its text (2.x's sentence's, else the item's label) and exactly its slot's request.
fn check_track(theirs: &[Value], step: usize, slots: Option<&Slots>, drawn: &[Drawn], flow: Option<&str>) -> Result<&'static str, String> {
    let (_, title, share) = progress(theirs, step, subtree_end(theirs, step))?;
    let mut failures = vec![];
    for (locale, order, current) in readings(&title, share, flow)? {
        let expected: Vec<Expected> = order.iter().enumerate().map(|(place, item)| {
            let state = match place.cmp(&current) { std::cmp::Ordering::Less => "done", std::cmp::Ordering::Equal => "current", _ => "upcoming" };
            let label = vec![deltabadger::web::i18n::text(locale, &format!("bot.setup.progress_steps.{}", ITEM_KEYS[*item]), &[])];
            match slots {
                // No sentence (the broker step): every item plain, its text not compared.
                None => (state.to_string(), None, None),
                Some(slots) => (state.to_string(), Some(slots.shown[*item].clone().unwrap_or(label)), slots.requests[*item].as_ref()),
            }
        }).collect();
        let fits = drawn.len() == 3 && drawn.iter().zip(&expected).all(|((state, shown, control), (want_state, want_shown, want_control))| {
            state == want_state && want_shown.as_ref().is_none_or(|w| w == shown) && control.as_ref() == *want_control
        });
        if fits { return Ok(locale) }
        failures.push(format!("{locale}: {expected:?}"));
    }
    Err(format!("the track is not the step's:\n  drawn: {drawn:?}\n  expected one of: {}", failures.join("\n    ")))
}

/// D-WIZARD-VENUE-GROUPS: this crate's exchange step draws its venues in two grids under "Brokers" and "Exchanges" where
/// 2.x draws one. Each group must keep 2.x's order and the two must hold exactly 2.x's tiles, under the locale's headings
/// (a group with no venue is not drawn); then this crate's groups are put back as 2.x's grid.
/// D-WIZARD-CONNECTED-GROUP: a first group "Connected" repeats venues of the groups below it (the ones the user holds a key
/// on); it must be first, under the locale's heading, and hold only tiles the other groups draw. It is then set aside.
fn ungroup(theirs: &[Value], step: usize, ours: &mut Vec<Value>, ours_step: usize, locale: &str, searched: bool) -> Result<(), String> {
    let Some(groups) = positions(ours, ours_step, subtree_end(ours, ours_step), |l| has_class(l, "div", "exchange-groups")).first().copied() else { return Ok(()) };
    let grid = positions(theirs, step, subtree_end(theirs, step), |l| has_class(l, "div", "exchange-grid")).first().copied().ok_or("venue groups where 2.x has no grid")?;
    let options = |lines: &[Value], at: usize| -> Vec<Vec<Value>> {
        children(lines, at, subtree_end(lines, at), depth(&lines[at])).into_iter().map(|o| without_space(&reindent(&lines[o..subtree_end(lines, o)], 0))).collect()
    };
    let v2 = options(theirs, grid);
    let groups_end = subtree_end(ours, groups);
    let (mut drawn, mut titles) = (vec![], vec![]);
    let mut connected: Option<Vec<Vec<Value>>> = None;
    // D-VENUE-TILE-ASKS-KEY: the form's `untargeted` input sits among the groups; it is no group.
    for (index, group) in children(ours, groups, groups_end, depth(&ours[groups])).into_iter().filter(|g| opens(&ours[*g], "section")).enumerate() {
        let end = subtree_end(ours, group);
        if has_class(&ours[group], "section", "exchange-group--connected") {
            let heading = positions(ours, group, end, |l| has_class(l, "h3", "label")).first().and_then(|h| words(&ours[h + 1])).unwrap_or_default();
            let grid = positions(ours, group, end, |l| has_class(l, "div", "exchange-grid")).first().copied().ok_or("the Connected group without its grid")?;
            if index != 0 || heading != deltabadger::web::i18n::text(locale, "bot.setup.venue_groups.connected", &[]) { return Err(format!("the Connected group is not first under its heading: {heading:?} at {index}")) }
            // D-VENUE-TILE-ASKS-KEY: a Connected tile posts its account (`formaction`), its number in a corner with 2+ live
            // accounts (never on a recorded, one-account install); otherwise it is the ordinary tile.
            connected = Some(options(ours, grid).into_iter().map(|tile| tile.into_iter().map(|l| match l.as_str() {
                Some(t) if opens(&l, "button") => Value::String(match t.find(" formaction=\"") {
                    Some(f) => { let v = f + " formaction=\"".len(); format!("{}{}", &t[..f], &t[v + t[v..].find('"').map_or(0, |e| e + 1)..]) }
                    None => t.to_string(),
                }),
                _ => l,
            }).collect()).collect());
            continue;
        }
        let heading = positions(ours, group, end, |l| has_class(l, "h3", "label")).first().and_then(|h| words(&ours[h + 1])).unwrap_or_default();
        let grid = positions(ours, group, end, |l| has_class(l, "div", "exchange-grid")).first().copied().ok_or("a venue group without its grid")?;
        titles.push(heading);
        drawn.push(options(ours, grid));
    }
    if let Some(connected) = connected {
        if connected.is_empty() || connected.iter().any(|tile| !drawn.iter().flatten().any(|o| o == tile)) { return Err("the Connected group is empty or holds a venue the groups below it do not".into()) }
    }
    let title = |key: &str| deltabadger::web::i18n::text(locale, &format!("bot.setup.venue_groups.{key}"), &[]);
    // No venue at all (nothing the basket trades on): no group, where 2.x's grid is empty.
    let want: Vec<String> = match drawn.len() { 2 => vec![title("brokers"), title("exchanges")], 0 | 1 => vec![], n => return Err(format!("{n} venue groups")) };
    if drawn.len() == 2 && titles != want { return Err(format!("the venue groups' headings: {titles:?}, not {want:?}")) }
    if drawn.len() == 1 && ![title("brokers"), title("exchanges")].contains(&titles[0]) { return Err(format!("the venue group's heading: {:?}", titles[0])) }
    // Each group in 2.x's order, and the two together exactly 2.x's tiles.
    let in_order = |group: &[Vec<Value>]| group.iter().map(|o| v2.iter().position(|r| r == o)).collect::<Option<Vec<_>>>().is_some_and(|at| at.windows(2).all(|w| w[0] < w[1]));
    // D-EXCHANGE-NO-SEARCH: 2.x's matches for a query, every one of them among this crate's venues.
    if searched { if v2.iter().any(|tile| !drawn.iter().flatten().any(|o| o == tile)) { return Err("2.x's matching venues are not all drawn".into()) } }
    else if !drawn.iter().all(|group| in_order(group)) || drawn.iter().map(Vec::len).sum::<usize>() != v2.len() {
        return Err("the venue groups are not 2.x's tiles, each group in 2.x's order".into());
    }
    let back = reindent(&theirs[grid..subtree_end(theirs, grid)], depth(&ours[groups]));
    ours.splice(groups..groups_end, back);
    Ok(())
}

/// D-WIZARD-SHEET: each basket row's name cell (`modal--search__cell--name`: the logo, then the name in its label) read back
/// as 2.x's (`modal--search__cell`, the name alone). A cell without its logo is not this crate's.
fn unlogo(ours: &mut Vec<Value>) -> Result<(), String> {
    while let Some(row) = positions(ours, 0, ours.len(), |l| has_class(l, "div", "wizard-assets__row")).into_iter()
        .find(|row| !positions(ours, *row, subtree_end(ours, *row), |l| has_class(l, "div", "modal--search__cell--name")).is_empty()) {
        let cell = positions(ours, row, subtree_end(ours, row), |l| has_class(l, "div", "modal--search__cell--name"))[0];
        let end = subtree_end(ours, cell);
        if positions(ours, cell, end, |l| attr(text(l), "class").is_some_and(|c| c.split(' ').any(|c| c == "asset-logo"))).is_empty() { return Err("a basket row without its logo".into()) }
        let label = positions(ours, cell, end, |l| has_class(l, "span", "modal--search__cell--name__label")).first().copied().ok_or("a basket row without its name")?;
        let name = ours[label + 1].clone();
        let indent = "  ".repeat(depth(&ours[cell]));
        let back = vec![json!(format!("{indent}<div class=\"modal--search__cell\">")), json!(format!("{}{}", "  ".repeat(depth(&ours[cell]) + 1), text(&name).trim_start())), json!(format!("{indent}</div>"))];
        ours.splice(cell..end, back);
    }
    Ok(())
}

/// D-WIZARD-SHEET-KEY-FORM: the key card's title is the venue's name alone (its button says "Connect"), where 2.x's reads
/// "Connect {name}" in the page's locale; read back as 2.x's.
fn retitle(theirs: &[Value], step: usize, ours: &mut [Value], ours_step: usize, locale: &str) -> Result<(), String> {
    let title = |lines: &[Value], at: usize| positions(lines, at, subtree_end(lines, at), |l| has_class(l, "h2", "set-api__title")).first().map(|t| t + 1);
    let (Some(mine), Some(v2)) = (title(ours, ours_step), title(theirs, step)) else { return Ok(()) };
    let name = words(&ours[mine]).unwrap_or_default();
    let want = deltabadger::web::i18n::text(locale, "bot.setup.connect_to", &[("name", deltabadger::web::i18n::Arg::Text(&name))]);
    if words(&theirs[v2]).as_deref() != Some(want.as_str()) { return Err(format!("the key card's title {name:?} is not 2.x's {:?}", words(&theirs[v2]))) }
    ours[mine] = json!(format!("{}{}", "  ".repeat(depth(&ours[mine])), text(&theirs[v2]).trim_start()));
    Ok(())
}

/// D-WIZARD-SHEET-KEY-FORM: the key step's form in Settings' rows and controls (`.srow`, `.rinput`, `.rbutton`) where 2.x
/// has its floating-label rows. Compared by what it submits and says: the form itself, every field (name, type, id, value
/// but the token's, autofocus, autocomplete, its controller, Connect's Turbo attributes) and every label with its field,
/// in order; then read back as 2.x's form.
fn reform(theirs: &[Value], step: usize, ours: &mut Vec<Value>, ours_step: usize) -> Result<(), String> {
    let form = |lines: &[Value], at: usize| positions(lines, at, subtree_end(lines, at), |l| has_class(l, "form", "set-api__form")).first().copied();
    let Some(mine) = form(ours, ours_step) else { return Ok(()) };
    let mine_end = subtree_end(ours, mine);
    if positions(ours, mine, mine_end, |l| has_class(l, "input", "rinput")).is_empty() { return Ok(()) }
    let v2 = form(theirs, step).ok_or("a key form where 2.x has none")?;
    let said = |lines: &[Value], at: usize| -> Vec<String> {
        let end = subtree_end(lines, at);
        let mut out = vec![text(&lines[at]).trim().to_string()];
        for value in &lines[at..end] {
            let line = text(value);
            if opens(value, "input") {
                let token = attr(line, "name").as_deref() == Some("authenticity_token");
                out.push(["name", "type", "id", "value", "autofocus", "autocomplete", "data-controller", "data-turbo-stream", "data-turbo-submits-with", "data-disable-with"].iter()
                    .filter(|a| !(token && **a == "value")).map(|a| format!("{a}={:?}", attr(line, a))).collect::<Vec<_>>().join(" "));
            }
        }
        // Each visible field's name for a reader: 2.x's <label for> (after its field), this crate's aria-label (Settings'
        // fields carry their label as placeholder and aria-label).
        let labels: Vec<(Option<String>, Option<String>)> = (at..end).filter(|i| opens(&lines[*i], "label")).map(|i| (attr(text(&lines[i]), "for"), words(&lines[i + 1]))).collect();
        out.extend((at..end).filter(|i| opens(&lines[*i], "input") && matches!(attr(text(&lines[*i]), "type").as_deref(), Some("text" | "password"))).map(|i| {
            let id = attr(text(&lines[i]), "id");
            let named = labels.iter().find(|(f, _)| *f == id).and_then(|(_, t)| t.clone()).or_else(|| attr(text(&lines[i]), "aria-label"));
            format!("field {id:?} named {named:?}")
        }));
        out
    };
    let (got, want) = (said(ours, mine), said(theirs, v2));
    if got != want { return Err(format!("the key form is not 2.x's:\n  drawn: {got:?}\n  2.x: {want:?}")) }
    let back = reindent(&theirs[v2..subtree_end(theirs, v2)], depth(&ours[mine]));
    ours.splice(mine..mine_end, back);
    Ok(())
}

/// The locale prefix of a path (`/de`), or none.
fn prefix(path: &str) -> &str {
    let path = path.split('?').next().unwrap_or_default();
    &path[..path.len() - route(path).len()]
}

const ASSET_STEP: &str = "/bots/dca_single_assets/pick_buyable_asset/new";
/// A switch option as drawn: href, frame, aria-current, label.
type Link = (Option<String>, Option<String>, Option<String>, Option<String>);

/// D-INDEX-IN-ASSETS: on the asset step (2.x's `wizard-assets` block) this crate's search row holds the Asset / Index switch
/// where 2.x's zoom icon is, and its form a hidden `kind=asset`. The switch must be exactly the two links (the step's full
/// path with `?kind=asset` and `?kind=index`, the locale prefix included, the frame `modal_content`, `aria-current` on Asset,
/// the locale's labels, no `data-segmented-frame`); then read back as 2.x's icon. At the cap 2.x draws no row and this
/// crate's row is the switch alone: removed. Any other step must have no switch.
fn unswitch(theirs: &[Value], step: usize, ours: &mut Vec<Value>, ours_step: usize, path: &str, locale: &str) -> Result<(), String> {
    // The search row's switch (`segmented--search`), not a form's own control (the Alpaca key's Paper / Live).
    let switch = |l: &Value| has_class(l, "div", "segmented--search");
    let ours_end = subtree_end(ours, ours_step);
    let switches = positions(ours, ours_step, ours_end, switch);
    let step_end = subtree_end(theirs, step);
    if positions(theirs, step, step_end, |l| has_class(l, "div", "wizard-assets")).is_empty() {
        return if switches.is_empty() { Ok(()) } else { Err("an Asset / Index switch off the asset step".into()) };
    }
    let [at] = switches[..] else { return Err(format!("{} Asset / Index switches on the asset step", switches.len())) };
    if attr(text(&ours[at]), "data-segmented-frame").is_some() { return Err("the switch manages page history (data-segmented-frame)".into()) }
    let end = subtree_end(ours, at);
    let label = |key: &str| deltabadger::web::i18n::text(locale, &format!("bot.setup.kind.{key}"), &[]);
    let links: Vec<Link> = positions(ours, at, end, |l| opens(l, "a")).iter()
        .map(|a| { let l = text(&ours[*a]); (attr(l, "href"), attr(l, "data-turbo-frame"), attr(l, "aria-current"), words(&ours[a + 1])) }).collect();
    let href = |kind: &str| Some(format!("{}{ASSET_STEP}?kind={kind}", prefix(path)));
    let want = vec![(href("asset"), Some("modal_content".into()), Some("page".into()), Some(label("asset"))),
                    (href("index"), Some("modal_content".into()), None, Some(label("index")))];
    if links != want { return Err(format!("the switch is not Asset / Index:\n  drawn: {links:?}\n  wanted: {want:?}")) }
    let row = |lines: &[Value], from: usize, to: usize| positions(lines, from, to, |l| has_class(l, "div", "modal--search") && !has_class(l, "div", "sheet__search")).first().copied();
    let ours_row = enclosing(ours, at, |l| has_class(l, "div", "modal--search")).ok_or("the switch outside the search row")?;
    match row(theirs, step, step_end) {
        // The cap: 2.x has no row; this crate's is the switch alone.
        None => {
            if children(ours, ours_row, subtree_end(ours, ours_row), depth(&ours[ours_row])) != [at] { return Err("the capped search row holds more than the switch".into()) }
            cut(ours, ours_row);
        }
        Some(row) => {
            let icon = positions(theirs, row, subtree_end(theirs, row), |l| opens(l, "svg")).first().copied().ok_or("2.x's search row has no icon")?;
            let form = enclosing(ours, at, |l| opens(l, "form")).ok_or("the switch outside the search form")?;
            let hidden: Vec<usize> = positions(ours, form, subtree_end(ours, form), |l| opens(l, "input") && attr(text(l), "name").as_deref() == Some("kind"));
            let [kind] = hidden[..] else { return Err(format!("{} kind fields in the search form", hidden.len())) };
            let l = text(&ours[kind]);
            if attr(l, "type").as_deref() != Some("hidden") || attr(l, "value").as_deref() != Some("asset") { return Err(format!("the search form's kind field: {l}")) }
            let back = reindent(&theirs[icon..subtree_end(theirs, icon)], depth(&ours[at]));
            // The field first (it comes after the switch) with the whitespace before it, then the switch.
            cut(ours, kind);
            ours.splice(at..end, back);
        }
    }
    Ok(())
}

/// D-WIZARD-SHEET on one answer to `path` (see the module). Ok(1) when it was a sheet put back as 2.x's step, Ok(0) when the
/// answer is not a DCA step's (not a DCA path, or no step in either body), Err when this crate's answer draws a step that is
/// not the sheet it must be.
pub fn wizard_sheet(path: &str, flow: Option<&str>, v2: &mut Value, rust: &mut Value) -> Result<usize, String> {
    if !dca_step(path) { return Ok(0) }
    let (Value::Array(theirs), Value::Array(ours)) = (v2, rust) else { return Ok(0) };
    let layout = |lines: &[Value]| positions(lines, 0, lines.len(), |l| has_class(l, "div", "bot-creation-layout"));
    let [step] = layout(theirs)[..] else { return Ok(0) };
    let modal = |l: &Value| opens(l, "turbo-frame") && attr(text(l), "id").as_deref() == Some("modal");
    let stream = |l: &Value| opens(l, "turbo-stream") && attr(text(l), "target").as_deref() == Some("modal_content");
    // 2.x's shell: the modal frame, or the stream's template.
    let (shell, streamed) = match (enclosing(theirs, step, modal), enclosing(theirs, step, stream)) {
        (Some(frame), None) => (frame, false),
        (None, Some(at)) => (enclosing(theirs, step, |l| opens(l, "template")).filter(|t| *t > at).ok_or("a stream without a template")?, true),
        _ => return Err("2.x's step is neither in the modal frame nor in a stream".into()),
    };
    // No step at all is another answer (a venue this build does not trade is refused); the comparison holds it as it did.
    if layout(ours).is_empty() { return Ok(0) }
    let ours_at = if streamed { positions(ours, 0, ours.len(), stream).first().and_then(|s| positions(ours, *s, subtree_end(ours, *s), |l| opens(l, "template")).first().copied()) }
                  else { positions(ours, 0, ours.len(), modal).first().copied() };
    let ours_shell = ours_at.ok_or("no sheet where 2.x has its step")?;
    // D-ASSETS-CAPTION: the Assets step's caption, first in the pane over either tab (2.x has none), then taken out.
    let first_step = positions(ours, ours_shell, subtree_end(ours, ours_shell), |l| has_class(l, "div", "bot-creation-layout")).first().copied().unwrap_or(ours.len());
    let captions = positions(ours, ours_shell, first_step, |l| has_class(l, "h3", "label"));
    if captions.len() > 1 { return Err(format!("{} captions over the step", captions.len())) }
    if let Some(at) = captions.first().copied() {
        if !path.contains("/pick_buyable_asset") { return Err(format!("a caption over the step on {path}")) }
        if words(&ours[at + 1]).is_none() { return Err("an empty caption".into()) }
        cut(ours, at);
    }
    let ours_end = subtree_end(ours, ours_shell);
    let [ours_step] = layout(&ours[ours_shell..ours_end])[..] else { return Err("this crate's sheet holds no step, or more than one".into()) };
    let ours_step = ours_shell + ours_step;
    let tracks_at = positions(ours, ours_shell, ours_end, |l| has_class(l, "ol", "sheet__track"));
    let [track_at] = tracks_at[..] else { return Err(format!("this crate's sheet holds {} tracks", tracks_at.len())) };
    // The track, against 2.x's progress bar and sentence.
    let step_end = subtree_end(theirs, step);
    let sentence = slots(theirs, step, step_end);
    let locale = check_track(theirs, step, sentence.as_ref().map(|(_, s)| s), &items(ours, track_at), flow)?;
    // The shell around the track and the step.
    let mut holes = [(track_at, "track"), (ours_step, "layout")];
    holes.sort();
    let ours_frame = without_space(&with_holes(ours, ours_shell, ours_end, &holes));
    let theirs_frame = without_space(&with_holes(theirs, shell, subtree_end(theirs, shell), &[(step, "layout")]));
    if streamed {
        let template = |inner: Vec<Value>| [vec![json!("<template>")], reindent(&inner, 1), vec![json!("</template>")]].concat();
        let sheet = without_space(&template(body_of(&html::normalize(&sheet_frame()))));
        if ours_frame != sheet { return Err(format!("the stream's template is not the sheet's frame: {ours_frame:?}")) }
        let v2_shell = without_space(&template(vec![json!("<x-layout>"), json!("</x-layout>")]));
        if theirs_frame != v2_shell { return Err(format!("2.x's stream holds more than its step: {theirs_frame:?}")) }
    } else {
        let close_url = positions(theirs, shell, subtree_end(theirs, shell), |l| opens(l, "dialog")).first().and_then(|d| attr(text(&theirs[*d]), "data-modal--base-close-url-value")).unwrap_or_default();
        let close = deltabadger::web::i18n::text(locale, "button.close", &[]);
        if ours_frame != without_space(&sheet_shell(&close_url, &close)) { return Err(format!("this crate's dialog is not the sheet: {ours_frame:?}")) }
        if theirs_frame != without_space(&full_page_shell(&close_url)) { return Err(format!("2.x's dialog is not the full-page shell: {theirs_frame:?}")) }
    }
    // The pane's search box and Next button: each exactly the form 2.x's sentence had for it, then taken out.
    let ours_step_end = subtree_end(ours, ours_step);
    let searches = positions(ours, ours_step, ours_step_end, |l| has_class(l, "div", "sheet__search"));
    let v2_search = sentence.as_ref().and_then(|(_, s)| s.search.as_ref());
    match (searches.as_slice(), v2_search) {
        ([], None) => {}
        // D-EXCHANGE-NO-SEARCH: the exchange step has none where 2.x's had one.
        ([], Some(_)) if path.contains("/pick_exchange") => {}
        ([at], Some(want)) => {
            let form = positions(ours, *at, subtree_end(ours, *at), |l| opens(l, "form"))[0];
            let got = request(ours, form);
            if got.method != want.method || got.url != want.url || got.frame != want.frame || got.search != want.search || !got.fields.is_empty() {
                return Err(format!("the pane's search is not the sentence's: {got:?} / {want:?}"));
            }
        }
        (found, want) => return Err(format!("{} search boxes in the pane where 2.x's sentence had {}", found.len(), if want.is_some() { "one" } else { "none" })),
    }
    let nexts = positions(ours, ours_step, ours_step_end, |l| has_class(l, "form", "sheet__next"));
    let advances = sentence.as_ref().map_or(&[][..], |(_, s)| s.advances.as_slice());
    match nexts.as_slice() {
        [] => {}
        [at] => {
            let got = request(ours, *at);
            if !advances.iter().any(|want| want.method == got.method && want.url == got.url && want.fields == got.fields && want.frame == got.frame) {
                return Err(format!("Next is not the sentence's advance: {got:?} / {advances:?}"));
            }
        }
        _ => return Err("more than one Next".into()),
    }
    // "Add more assets" under Next: the locale's words, and only where Next is.
    // The step's captions are .label, told apart by their words.
    let labelled = |key: &str| { let want = deltabadger::web::i18n::text(locale, key, &[]);
        positions(ours, ours_step, ours_step_end, |l| has_class(l, "h3", "label")).into_iter().filter(|at| words(&ours[at + 1]).as_deref() == Some(want.as_str())).collect::<Vec<_>>() };
    let more = labelled("bot.setup.add_more_assets");
    if more.len() != nexts.len() { return Err(format!("{} \"Add more assets\" labels for {} Next", more.len(), nexts.len())) }
    for at in nexts.iter().chain(&searches).chain(&more).copied().collect::<std::collections::BTreeSet<_>>().into_iter().rev() { cut(ours, at) }
    // A basket row's name cell holds the asset's logo, as a result row does; read back as 2.x's cell, the name alone.
    unlogo(ours)?;
    unrestyle(ours);
    resettle_search(ours)?;
    let ours_end = subtree_end(ours, ours_shell);
    let ours_step = positions(ours, ours_shell, ours_end, |l| has_class(l, "div", "bot-creation-layout"))[0];
    // D-WIZARD-VENUE-GROUPS: the brokers and the exchanges under their headings; read back as 2.x's one grid.
    ungroup(theirs, step, ours, ours_step, locale, searched(path))?;
    // The reason a key was refused, under the fields where 2.x flashes it (taken out with the form below).
    let refused = positions(ours, ours_step, subtree_end(ours, ours_step), |l| has_class(l, "p", "sform__error")).first().and_then(|p| words(&ours[p + 1]));
    unback(ours, path)?;
    retitle(theirs, step, ours, ours_step, locale)?;
    reform(theirs, step, ours, ours_step)?;
    unswitch(theirs, step, ours, ours_step, path, locale)?;
    let ours_end = subtree_end(ours, ours_shell);
    // The basket is drawn open; 2.x's was closed unless asked for.
    let list = |l: &Value| opens(l, "div") && attr(text(l), "class").is_some_and(|c| c.split(' ').any(|c| c == "wizard-assets__list"));
    if positions(ours, ours_step, subtree_end(ours, ours_step), list).iter().any(|at| attr(text(&ours[*at]), "class").is_some_and(|c| c.split(' ').any(|c| c == "hidden"))) {
        return Err("this crate's basket is hidden".into());
    }
    for at in positions(theirs, step, step_end, list) { theirs[at] = json!(text(&theirs[at]).replacen("class=\"wizard-assets__list hidden\"", "class=\"wizard-assets__list \"", 1)) }
    // 2.x's flash of that reason, which this crate's step says itself: the same words, then left out.
    if let Some(reason) = refused {
        let flash = positions(theirs, 0, theirs.len(), |l| opens(l, "turbo-stream") && attr(text(l), "target").as_deref() == Some("flash")).first().copied()
            .ok_or("a refusal under the fields where 2.x flashed nothing")?;
        if !(flash..subtree_end(theirs, flash)).any(|i| words(&theirs[i]).as_deref() == Some(reason.as_str())) {
            return Err(format!("the refusal under the fields, {reason:?}, is not 2.x's flash"));
        }
        cut(theirs, flash);
    }
    // 2.x's step without its progress bar and its sentence (and the whitespace before each).
    if let Some((at, _)) = sentence { cut(theirs, at) }
    let (bar, _, _) = progress(theirs, step, subtree_end(theirs, step))?;
    cut(theirs, bar);
    let (step_end, shell_end) = (subtree_end(theirs, step), subtree_end(theirs, shell));
    // This crate's shell is 2.x's, around this crate's step.
    let rebuilt = [theirs[shell..step].to_vec(), reindent(&ours[ours_step..subtree_end(ours, ours_step)], depth(&theirs[step])), theirs[step_end..shell_end].to_vec()].concat();
    ours.splice(ours_shell..ours_end, rebuilt);
    Ok(1)
}

/// D-WIZARD-SHEET, D-INDEX-IN-ASSETS and D-CHOOSER-SKIPPED on `GET /bots/new`, on whole answers (status, headers, body,
/// session). With advanced bots 2.x's chooser loses its Index tile (required to be the `setup_coingecko/new` link) and this
/// crate's DCA tile, which opens the sheet in the modal frame (`data-turbo-frame="modal"`, no `data-wizard-dir`), is put
/// back as 2.x's. Without them this crate answers 302 to the exchange step (where 2.x's DCA tile led) with the wizard begun
/// as 2.x begins it, and 2.x's chooser must be the two tiles DCA and Index: then this crate's answer reads as 2.x's.
/// Ok(1) when it was the chooser, Ok(0) for another answer (signed out), Err when this crate's answer is in another shape.
pub fn chooser(path: &str, theirs: &mut Value, ours: &mut Value) -> Result<usize, String> {
    if route(path) != "/bots/new" || theirs["status"] != 200 { return Ok(0) }
    let tile = |l: &Value| has_class(l, "a", "itile");
    let index_tile = |l: &Value| tile(l) && attr(text(l), "href").is_some_and(|h| h.ends_with("/bots/dca_indexes/setup_coingecko/new"));
    let Value::Array(v2) = &mut theirs["body"] else { return Err("2.x's chooser is not a page".into()) };
    let tiles = positions(v2, 0, v2.len(), tile).len();
    let [index] = positions(v2, 0, v2.len(), index_tile)[..] else { return Err("2.x's chooser has no Index tile".into()) };
    if ours["status"] == 302 {
        // The step in the page's locale (the path's, else the user's: the default one has no prefix).
        let want = "/bots/dca_single_assets/pick_exchange/new";
        if tiles != 2 { return Err(format!("this crate skipped a chooser of {tiles} tiles")) }
        if ours["headers"]["location"].as_str().is_none_or(|l| route(l) != want) { return Err(format!("the chooser skipped to {}, not {want}", ours["headers"]["location"])) }
        if ours.get("session").is_some_and(|s| s["bot_config"] != json!({}) || Some(s) != theirs.get("session")) { return Err(format!("the wizard begun as {}, not {{}}", ours["session"])) }
        *ours = theirs.clone();
        return Ok(1);
    }
    if tiles != 3 { return Err(format!("this crate drew the chooser where 2.x's has {tiles} tiles")) }
    cut(v2, index);
    let Value::Array(lines) = &mut ours["body"] else { return Err("this crate's chooser is not a page".into()) };
    if !positions(lines, 0, lines.len(), |l| tile(l) && attr(text(l), "href").is_some_and(|h| h.contains("/bots/dca_indexes/"))).is_empty() {
        return Err("this crate's chooser has an Index tile".into());
    }
    let dca = |l: &Value| tile(l) && attr(text(l), "href").is_some_and(|h| h.ends_with("/bots/dca_single_assets/pick_exchange/new"));
    let [at] = positions(lines, 0, lines.len(), dca)[..] else { return Err("this crate's chooser has no DCA tile".into()) };
    let line = text(&lines[at]).to_string();
    if !line.contains(" data-turbo-frame=\"modal\" ") || line.contains("data-wizard-dir") { return Err(format!("the DCA tile does not open the sheet: {line}")) }
    lines[at] = json!(line.replacen(" data-turbo-frame=\"modal\" ", " data-turbo-frame=\"modal_content\" data-wizard-dir=\"enter\" ", 1));
    Ok(1)
}

/// D-SHEET-EVERYWHERE, D-TRACKER-SHEET, D-SIGNAL-SHEET, D-RULES-SHEET, D-SHEET-NO-DIALOG (PLAN-sheet-wizard-rest): the tracker's
/// connect steps, the signal wizard, the withdrawal-rule wizard, the bot-type chooser and the bot menu's index picker are drawn
/// in the sheet where 2.x draws its full-page dialog (or, for a tracker key link, its modal card). Only the bodies that changed
/// are exempted, each under a structural check, so status, headers, session, venue requests and rows keep being compared:
/// - a step page: 2.x's modal frame and ours, ours the sheet (one track, none on the chooser and the picker) closing where
///   2.x's closes, every POST action of 2.x's step present in ours, the offered venues the same (`venues`); then each side's
///   modal frame is replaced by what stays compared: the chooser's and the picker's content, the signal rules editor
///   (`#new-settings`), the rule's preview (`#rule-preview`), or nothing;
/// - a key refused in the sheet: 2.x's flash and ours' sheet frame with the same reason under its fields; both replaced;
/// - the signal confirm step's POST: the Start button's class alone.
///
/// Returns how many answers it adapted.
pub fn sheet_flows(path: &str, method: &str, v2: &mut Value, rust: &mut Value) -> Result<usize, String> {
    let route = route(path);
    let tracker = route.starts_with("/tracker/pick_exchange") || route.starts_with("/tracker/add_api_key");
    let signal = route.starts_with("/bots/signals/");
    let rules = ["pick_exchange", "add_api_key", "pick_asset", "add_address", "confirm_settings"].iter().any(|step| route.starts_with(&format!("/rules/withdrawals/{step}")));
    let bare = route == "/bots/new" || (route.starts_with("/bots/") && route.ends_with("/index/new"));
    if !(tracker || signal || rules || bare) { return Ok(0) }
    let (Value::Array(theirs), Value::Array(ours)) = (v2, rust) else { return Ok(0) };
    if signal && method == "POST" && route == "/bots/signals/confirm_settings" { return start_button(theirs, ours) }
    let modal = |l: &Value| opens(l, "turbo-frame") && attr(text(l), "id").as_deref() == Some("modal");
    let refusal = |l: &Value| opens(l, "turbo-stream") && attr(text(l), "action").as_deref() == Some("update") && attr(text(l), "target").as_deref() == Some("modal_content");
    if let Some(at) = positions(ours, 0, ours.len(), refusal).first().copied() {
        // 2.x flashes the reason (the signal step also redraws its old card); ours says it under the sheet's fields.
        let inline = !positions(ours, at, subtree_end(ours, at), |l| has_class(l, "p", "sform__error")).is_empty();
        if (tracker || signal || rules) && method == "POST" && inline { return refused(theirs, ours, at) }
        return Ok(0)
    }
    let (Some(their_frame), Some(our_frame)) = (positions(theirs, 0, theirs.len(), modal).first().copied(), positions(ours, 0, ours.len(), modal).first().copied()) else { return Ok(0) };
    let (their_end, our_end) = (subtree_end(theirs, their_frame), subtree_end(ours, our_frame));
    let sheet = positions(ours, our_frame, our_end, |l| has_class(l, "turbo-frame", "sheet__frame") && attr(text(l), "id").as_deref() == Some("modal_content"));
    if sheet.len() != 1 { return Err(format!("{route}: no sheet where 2.x draws its dialog")) }
    let tracks = positions(ours, our_frame, our_end, |l| has_class(l, "ol", "sheet__track")).len();
    if tracks != usize::from(!bare) { return Err(format!("{route}: the sheet holds {tracks} tracks")) }
    let close = |lines: &[Value], from: usize, to: usize| positions(lines, from, to, |l| opens(l, "dialog")).first().and_then(|d| attr(text(&lines[*d]), "data-modal--base-close-url-value"));
    let ours_close = close(ours, our_frame, our_end).ok_or("the sheet has no close URL")?;
    match close(theirs, their_frame, their_end) {
        Some(theirs_close) if theirs_close != ours_close => return Err(format!("{route}: the sheet closes to {ours_close}, 2.x's dialog to {theirs_close}")),
        None if !(tracker && ours_close.ends_with("/tracker")) => return Err(format!("{route}: 2.x's card has no close URL and the sheet closes to {ours_close}")),
        _ => {}
    }
    let actions = |lines: &[Value], from: usize, to: usize| positions(lines, from, to, |l| opens(l, "form") && attr(text(l), "method").as_deref() == Some("post"))
        .into_iter().filter_map(|f| attr(text(&lines[f]), "action")).collect::<Vec<_>>();
    let ours_actions = actions(ours, our_frame, our_end);
    if let Some(missing) = actions(theirs, their_frame, their_end).into_iter().find(|a| !ours_actions.contains(a)) { return Err(format!("{route}: 2.x's step posts to {missing}, the sheet does not")) }
    venues(route, searched(path), &theirs[their_frame..their_end], &ours[our_frame..our_end])?;
    let kept = |lines: &[Value], from: usize, to: usize| -> Result<Vec<Value>, String> {
        let keep = |l: &Value| opens(l, "div") && matches!(attr(text(l), "id").as_deref(), Some("new-settings" | "rule-preview"));
        if bare {
            let inner = positions(lines, from, to, |l| has_class(l, "div", "bot-creation-layout") || has_class(l, "div", "index-picker-layout")).first().copied().ok_or("no content in the dialog")?;
            return Ok(reindent(&lines[inner..subtree_end(lines, inner)], depth(&lines[from])));
        }
        Ok(match positions(lines, from, to, keep).first() { Some(at) => reindent(&lines[*at..subtree_end(lines, *at)], depth(&lines[from])), None => vec![json!(format!("{}<x-sheet-step></x-sheet-step>", "  ".repeat(depth(&lines[from]))))] })
    };
    let (theirs_kept, ours_kept) = (kept(theirs, their_frame, their_end)?, kept(ours, our_frame, our_end)?);
    theirs.splice(their_frame..their_end, theirs_kept);
    ours.splice(our_frame..our_end, ours_kept);
    Ok(1)
}

/// D-BOT-KEY-SHEET, D-TRACKER-COINGECKO-SHEET (PLAN-key-cards-sheet): the bot page's key step and the tracker's CoinGecko
/// step are the sheet's card with no track where 2.x draws its modal card (2.x's has no close URL; ours closes to the bot or the
/// tracker). Exempted only under a form contract: each of 2.x's POST forms is in ours with the same action, `_method`, a CSRF
/// field, the same named fields, and Alpaca's Paper/Live value. A refused key is one `update modal_content` stream whose form
/// posts where the request posted, with a CSRF field, no `_method`, exactly the fields the request posted (`posted`, the step's
/// form) and the Paper/Live value it posted; its reason's words those 2.x flashed. Returns how many answers it adapted.
pub fn key_cards(path: &str, method: &str, posted: &Value, v2: &mut Value, rust: &mut Value) -> Result<usize, String> {
    let route = route(path);
    let bot = route.strip_prefix("/bots/").and_then(|rest| rest.split_once('/')).filter(|(id, step)| id.parse::<u64>().is_ok() && matches!(*step, "add_api_key" | "add_api_key/new"));
    let coingecko = matches!(route, "/tracker/connect_market_data" | "/tracker/export_modal" | "/tracker/setup_coingecko");
    if bot.is_none() && !coingecko { return Ok(0) }
    let (Value::Array(theirs), Value::Array(ours)) = (v2, rust) else { return Ok(0) };
    let marker = |lines: &[Value], at: usize, tag: &str| json!(format!("{}<{tag}></{tag}>", "  ".repeat(depth(&lines[at]))));
    if method == "POST" {
        let streams = positions(ours, 0, ours.len(), |l| opens(l, "turbo-stream"));
        let [at] = streams[..] else { return if streams.is_empty() { Ok(0) } else { Err(format!("{route}: the refusal is {} streams", streams.len())) } };
        if attr(text(&ours[at]), "action").as_deref() != Some("update") || attr(text(&ours[at]), "target").as_deref() != Some("modal_content") { return Ok(0) }
        let end = subtree_end(ours, at);
        if !positions(ours, at, end, |l| has_class(l, "ol", "sheet__track")).is_empty() { return Err(format!("{route}: the refused card has a track")) }
        let mut fields: Vec<String> = posted.as_object().map(|form| form.keys().filter(|k| k.starts_with("api_key")).cloned().collect()).unwrap_or_default();
        fields.sort();
        let mode = posted.get("api_key[passphrase]").and_then(Value::as_str).filter(|v| matches!(*v, "paper" | "live")).map(str::to_string);
        let forms = form_contracts(ours, at, end);
        let [form] = &forms[..] else { return Err(format!("{route}: the refused card has {} forms", forms.len())) };
        if route_of(&form.action) != route || !form.csrf || form.method.is_some() || form.fields != fields || form.mode != mode {
            return Err(format!("{route}: the refused card's form {form:?} is not the one posted ({fields:?}, {mode:?})"))
        }
        let error = positions(ours, at, end, |l| has_class(l, "p", "sform__error")).first().copied().ok_or(format!("{route}: no reason under the fields"))?;
        let reason = (error + 1..subtree_end(ours, error)).filter_map(|i| words(&ours[i])).collect::<Vec<_>>().join(" ");
        let flashed = theirs.iter().filter_map(words).collect::<Vec<_>>().join(" ");
        if reason.is_empty() || reason != flashed { return Err(format!("{route}: the sheet's reason \"{reason}\" is not 2.x's \"{flashed}\"")) }
        *theirs = vec![json!("<x-key-refusal></x-key-refusal>")];
        *ours = vec![json!("<x-key-refusal></x-key-refusal>")];
        return Ok(1)
    }
    let modal = |l: &Value| opens(l, "turbo-frame") && attr(text(l), "id").as_deref() == Some("modal");
    let (Some(their_frame), Some(our_frame)) = (positions(theirs, 0, theirs.len(), modal).first().copied(), positions(ours, 0, ours.len(), modal).first().copied()) else { return Ok(0) };
    let (their_end, our_end) = (subtree_end(theirs, their_frame), subtree_end(ours, our_frame));
    // The export modal with market data configured is not a card: compared as it is.
    if positions(ours, our_frame, our_end, |l| has_class(l, "turbo-frame", "sheet__frame")).len() != 1 { return if route == "/tracker/export_modal" { Ok(0) } else { Err(format!("{route}: no sheet where 2.x draws its card")) } }
    if !positions(ours, our_frame, our_end, |l| has_class(l, "ol", "sheet__track")).is_empty() { return Err(format!("{route}: the card's sheet has a track")) }
    let close = positions(ours, our_frame, our_end, |l| opens(l, "dialog")).first().and_then(|d| attr(text(&ours[*d]), "data-modal--base-close-url-value")).ok_or("the sheet has no close URL")?;
    let want = bot.map_or("/tracker".to_string(), |(id, _)| format!("/bots/{id}"));
    if route_of(&close) != want { return Err(format!("{route}: the sheet closes to {close}, not {want}")) }
    let ours_forms = form_contracts(ours, our_frame, our_end);
    for form in form_contracts(theirs, their_frame, their_end) {
        let Some(mine) = ours_forms.iter().find(|f| f.action == form.action) else { return Err(format!("{route}: 2.x's card posts to {}, the sheet does not", form.action)) };
        if mine != &form { return Err(format!("{route}: the sheet's form {mine:?} is not 2.x's {form:?}")) }
    }
    let (theirs_marker, ours_marker) = (marker(theirs, their_frame, "x-key-card"), marker(ours, our_frame, "x-key-card"));
    theirs.splice(their_frame..their_end, [theirs_marker]);
    ours.splice(our_frame..our_end, [ours_marker]);
    Ok(1)
}

/// What a POST form submits: its action, `_method`, whether it carries the CSRF field, its named fields (sorted) and the
/// Paper/Live value it sends.
#[derive(Debug, PartialEq)]
struct FormContract { action: String, method: Option<String>, csrf: bool, fields: Vec<String>, mode: Option<String> }

fn form_contracts(lines: &[Value], from: usize, to: usize) -> Vec<FormContract> {
    positions(lines, from, to, |l| opens(l, "form") && attr(text(l), "method").as_deref() == Some("post")).into_iter().map(|f| {
        let inputs = positions(lines, f, subtree_end(lines, f), |l| opens(l, "input"));
        let value = |name: &str| inputs.iter().find(|i| attr(text(&lines[**i]), "name").as_deref() == Some(name)).and_then(|i| attr(text(&lines[*i]), "value"));
        let mut fields: Vec<String> = inputs.iter().filter_map(|i| attr(text(&lines[*i]), "name")).filter(|n| !matches!(n.as_str(), "authenticity_token" | "_method" | "commit")).collect();
        fields.sort();
        fields.dedup();
        FormContract { action: attr(text(&lines[f]), "action").unwrap_or_default(), method: value("_method"), csrf: value("authenticity_token").is_some(), fields, mode: value("api_key[passphrase]").filter(|v| matches!(v.as_str(), "paper" | "live")) }
    }).collect()
}

fn route_of(path: &str) -> String { route(path).to_string() }

/// The venues a venue step offers: each pick's button (`name` ending in `exchange_id` or `exchange_id]`) and its value.
fn offered(lines: &[Value]) -> Vec<String> {
    lines.iter().filter(|l| opens(l, "button")).filter(|l| attr(text(l), "name").is_some_and(|n| n.ends_with("exchange_id") || n.ends_with("exchange_id]")))
        .filter_map(|l| attr(text(l), "value")).collect()
}

/// A venue step offers 2.x's venues less the flow's named exclusions (the rules picker: D-RULE-BINANCE-US-UNSUPPORTED, Binance
/// US, id 5 on the recorded install), each of this crate's groups (Brokers, Exchanges) in 2.x's relative order.
fn venues(route: &str, searched: bool, theirs: &[Value], ours: &[Value]) -> Result<(), String> {
    let mut expected = offered(theirs);
    if route == "/rules/withdrawals/pick_exchange/new" {
        if !expected.iter().any(|id| id == "5") { return Err("D-RULE-BINANCE-US-UNSUPPORTED: 2.x's picker no longer offers Binance US".into()) }
        expected.retain(|id| id != "5");
    }
    let groups = positions(ours, 0, ours.len(), |l| has_class(l, "section", "exchange-group"));
    // D-WIZARD-CONNECTED-GROUP: the first group may repeat venues of the others; it is checked against them and set aside.
    let is_connected = |g: &usize| has_class(&ours[*g], "section", "exchange-group--connected");
    if groups.iter().skip(1).any(is_connected) { return Err(format!("{route}: the Connected group is not first")) }
    let connected: Vec<String> = groups.iter().take(1).filter(|g| is_connected(g)).flat_map(|g| offered(&ours[*g..subtree_end(ours, *g)])).collect();
    let groups: Vec<usize> = groups.into_iter().filter(|g| !is_connected(g)).collect();
    let drawn: Vec<Vec<String>> = if groups.is_empty() && connected.is_empty() { vec![offered(ours)] } else { groups.iter().map(|g| offered(&ours[*g..subtree_end(ours, *g)])).collect() };
    if let Some(id) = connected.iter().find(|id| !drawn.iter().flatten().any(|o| o == *id)) { return Err(format!("{route}: connected venue {id} is not among the venues below")) }
    let mut all: Vec<String> = drawn.concat();
    let mut sorted = expected.clone();
    sorted.sort(); all.sort();
    // D-EXCHANGE-NO-SEARCH: a query's matches (2.x's) all among the sheet's venues.
    if searched { if let Some(id) = expected.iter().find(|id| !all.contains(id)) { return Err(format!("{route}: 2.x's match {id} is not offered")) } }
    else if sorted != all { return Err(format!("{route}: the sheet offers {:?}, 2.x {:?}", drawn.concat(), expected)) }
    for group in drawn.iter().filter(|_| !searched) {
        let order: Vec<&String> = expected.iter().filter(|id| group.contains(id)).collect();
        if order != group.iter().collect::<Vec<_>>() { return Err(format!("{route}: a venue group out of 2.x's order: {group:?} against {expected:?}")) }
    }
    Ok(())
}

/// A key refused in the sheet (D-WIZARD-SHEET-KEY-FORM): ours the step again with the reason under its fields, theirs the reason
/// flashed. The reason's text must be the same; then both answers are one marker.
fn refused(theirs: &mut Vec<Value>, ours: &mut Vec<Value>, at: usize) -> Result<usize, String> {
    let end = subtree_end(ours, at);
    let error = positions(ours, at, end, |l| has_class(l, "p", "sform__error")).first().copied().ok_or("a sheet refusal with no reason under its fields")?;
    let reason = (error + 1..subtree_end(ours, error)).filter_map(|i| words(&ours[i])).collect::<Vec<_>>().join(" ");
    if reason.is_empty() || !theirs.iter().filter_map(words).collect::<Vec<_>>().join(" ").contains(&reason) { return Err(format!("the sheet's reason \"{reason}\" is not 2.x's flash")) }
    *theirs = vec![json!("<x-sheet-refusal></x-sheet-refusal>")];
    *ours = vec![json!("<x-sheet-refusal></x-sheet-refusal>")];
    Ok(1)
}

/// D-SIGNAL-SHEET: the confirm step's Start is the sheet's primary button; in the `create-bot-button` stream only its class differs.
fn start_button(theirs: &mut [Value], ours: &mut [Value]) -> Result<usize, String> {
    let submit = |l: &Value| opens(l, "input") && attr(text(l), "name").as_deref() == Some("commit");
    let (Some(a), Some(b)) = (positions(theirs, 0, theirs.len(), submit).first().copied(), positions(ours, 0, ours.len(), submit).first().copied()) else { return Ok(0) };
    let line = text(&ours[b]).to_string();
    if !line.contains("class=\"rbutton rbutton--primary\"") || !text(&theirs[a]).contains("class=\"sbutton button--success\"") { return Err(format!("the Start button is not the sheet's: {line}")) }
    ours[b] = json!(line.replacen("class=\"rbutton rbutton--primary\"", "class=\"sbutton button--success\"", 1));
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSRF: &str = "<input type=\"hidden\" name=\"authenticity_token\" value=\"t\" />";
    const ORDER: &str = "/bots/dca_single_assets/order";
    const ADVANCE: &str = "/bots/dca_single_assets/pick_buyable_asset/advance";
    const EXCHANGES: &str = "/bots/dca_single_assets/pick_exchange/new";
    const ASSETS: &str = "/bots/dca_single_assets/pick_buyable_asset/new";

    fn bar(title: &str, share: usize) -> String {
        format!("\n<div class=\"process-progress\">\n  <h4>{title}</h4>\n  <div class=\"slider\"><div class=\"slider__style\"><div class=\"slider__style__track\"><div class=\"slider__style__progress\" style=\"width: {share}%\"></div></div></div></div>\n</div>")
    }
    fn post(class: &str, url: &str, fields: &[(&str, &str)], dir: bool, shown: &str) -> String {
        let fields: String = fields.iter().map(|(n, v)| format!("<input value=\"{v}\" type=\"hidden\" name=\"{n}\" />")).collect();
        format!("<form class=\"{class}\" data-turbo-frame=\"modal_content\"{} action=\"{url}\" accept-charset=\"UTF-8\" method=\"post\">{CSRF}{fields}<input {shown} readonly tabindex=\"-1\"></form>", if dir { " data-wizard-dir=\"back\"" } else { "" })
    }
    /// 2.x's sentence on the exchange-first key step with nothing chosen: the venue filled (back), the assets' order switch.
    fn key_sentence() -> String {
        format!("<div class=\"conversational conversational--bot-setup\"><span class=\"conversational__lead\">Buy</span><div class=\"conversational__assets\">{}</div><span class=\"conversational__lead\">on</span><form class=\"conversational__link filled\" data-turbo-frame=\"modal_content\" data-wizard-dir=\"back\" action=\"{EXCHANGES}\" accept-charset=\"UTF-8\" method=\"get\"><input value=\"Kraken\" readonly tabindex=\"-1\"></form></div>",
                post("ticker ticker--placeholder ticker--switch", ORDER, &[("flow", "asset_first")], false, "placeholder=\"\""))
    }
    /// 2.x's sentence on the asset step, exchange first, BTC and ETH chosen on Kraken: the venue goes back by the advance with
    /// `to=exchange`, the quote advances.
    fn asset_sentence() -> String {
        format!("<div class=\"conversational conversational--bot-setup\"><span class=\"conversational__lead\">Buy</span><div class=\"conversational__assets\"><div class=\"conversational__stack conversational__stack--toggle\"><div class=\"ticker-group ticker-group--2\"><div class=\"ticker\" data-ticker-symbol=\"BTC\">BTC</div><div class=\"ticker\" data-ticker-symbol=\"ETH\">ETH</div></div></div></div><span class=\"conversational__lead\">on</span>{}<span class=\"conversational__lead\">spending</span>{}</div>",
                post("conversational__link filled", ADVANCE, &[("to", "exchange")], true, "value=\"Kraken\""), post("ticker ticker--placeholder ticker--advance", ADVANCE, &[], false, "placeholder=\"\""))
    }
    fn invite_sentence() -> String {
        format!("<div class=\"conversational conversational--bot-setup\"><span class=\"conversational__lead\">You can also start with</span>{}</div>",
                post("conversational__link conversational__link--placeholder conversational__link--switch", ORDER, &[("flow", "asset_first")], false, "placeholder=\"assets\""))
    }
    fn body(inner: &str) -> String { format!("<div class=\"vt-step-body\"><form action=\"/x\"><input name=\"field\" value=\"1\"></form>{inner}</div>") }
    fn basket(hidden: bool) -> String {
        format!("<div class=\"wizard-assets__list {}\" data-class-toggle-target=\"togglable\"><div class=\"wizard-assets__row\"><div class=\"modal--search__cell\">Bitcoin</div></div></div>", if hidden { "hidden" } else { "" })
    }
    /// This crate's basket: the same row, its name cell holding the asset's logo.
    fn ours_basket(logo: bool) -> String {
        format!("<div class=\"wizard-assets__list \" data-class-toggle-target=\"togglable\"><div class=\"wizard-assets__row\"><div class=\"modal--search__cell modal--search__cell--name\">{}<span class=\"modal--search__cell--name__label\">Bitcoin</span></div></div></div>", if logo { "<span class=\"asset-logo\"></span>" } else { "" })
    }
    fn v2_step(title: &str, share: usize, sentence: &str, inner: &str) -> String {
        format!("<div class=\"bot-creation-layout\">\n    {}\n\n    {sentence}\n\n    {}\n  </div>\n", bar(title, share), body(inner))
    }
    fn ours_step(extras: &str, inner: &str) -> String { format!("<div class=\"bot-creation-layout\">\n    {extras}\n\n    {}\n  </div>\n", body(inner)) }
    fn page(modal: &str) -> Value { json!(html::normalize(&format!("<html><body><main>bots</main>{modal}</body></html>"))) }
    fn v2_page(content: &str) -> Value {
        // As v2_answer has it, the backdrop's click put on (D-FULL-PAGE-DIALOG-BACKDROP-CLOSES).
        page(&include_str!("v2_full_page.html").replace("{{ close_url }}", "/bots").replace("{% include \"svg/_24x24_close.html\" %}", include_str!("../../templates/svg/_24x24_close.html"))
            .replace("{{ content|safe }}", content).replacen(" complete=\"\"", "", 1))
    }
    /// One track item as _track.html draws it: `action` None (plain), ("get", url, dir) or ("post", url, fields, dir).
    /// A track item's request in a fixture: method, URL, hidden fields, back.
    type Act<'a> = Option<(&'a str, &'a str, &'a [(&'a str, &'a str)], bool)>;
    fn item(state: &str, shown: &str, action: Act) -> String {
        let (class, aria) = match state { "done" => (" sheet__step--done", ""), "current" => (" sheet__step--current", " aria-current=\"step\""), _ => ("", "") };
        let label = format!("<span class=\"sheet__label\">{shown}</span>");
        let control = match action {
            None => format!("<span class=\"sheet__control\">{label}</span>"),
            Some(("get", url, _, dir)) => format!("<a class=\"sheet__control\" href=\"{url}\" data-turbo-frame=\"modal_content\"{}>{label}</a>", if dir { " data-wizard-dir=\"back\"" } else { "" }),
            Some((_, url, fields, dir)) => {
                let fields: String = fields.iter().map(|(n, v)| format!("<input type=\"hidden\" name=\"{n}\" value=\"{v}\" />")).collect();
                format!("<form class=\"button_to sheet__control-form\" method=\"post\" action=\"{url}\" data-turbo-frame=\"modal_content\"{}><button class=\"sheet__control\" type=\"submit\">{label}</button>{CSRF}{fields}</form>", if dir { " data-wizard-dir=\"back\"" } else { "" })
            }
        };
        format!("\n  <li class=\"sheet__step{class}\"{aria}>{control}</li>")
    }
    fn track(items: &[String]) -> String { format!("<ol class=\"sheet__track\">{}\n</ol>", items.concat()) }
    fn sheet(frame_id: &str, track: &str, content: &str) -> String {
        let frame = include_str!("../../templates/wizard/_sheet_frame.html").replace("{{ track|safe }}", track).replace("{{ content|safe }}", content);
        include_str!("../../templates/wizard/sheet.html").replace("{{ close_url }}", "/bots").replace("{{ close_label }}", &deltabadger::web::i18n::text("en", "button.close", &[]))
            .replace("{% include \"svg/_24x24_close.html\" %}", include_str!("../../templates/svg/_24x24_close.html"))
            .replace("{% include \"wizard/_sheet_frame.html\" %}", &frame).replace("id=\"modal_content\"", &format!("id=\"{frame_id}\""))
    }
    fn same(theirs: &Value, ours: &Value) -> bool { without_space(theirs.as_array().unwrap()) == without_space(ours.as_array().unwrap()) }
    fn search(frame: &str, url: &str, placeholder: &str) -> String {
        format!("<div class=\"modal--search sheet__search\"><form data-turbo-frame=\"{frame}\" action=\"{url}\" method=\"get\"><input autofocus=\"autofocus\" placeholder=\"{placeholder}\" type=\"text\" name=\"query\" id=\"query\" /></form></div>")
    }
    fn next(fields: &str) -> String { format!("<form class=\"button_to sheet__next\" data-turbo-frame=\"modal_content\" method=\"post\" action=\"{ADVANCE}\"><button type=\"submit\">Next</button>{CSRF}{fields}</form><h3 class=\"label\">Add more assets</h3>") }
    fn asset_track(exchange_fields: &[(&str, &str)]) -> String {
        track(&[item("done", "Kraken", Some(("post", ADVANCE, exchange_fields, true))), item("current", "BTC</span><span>ETH", None), item("upcoming", "Currency", Some(("post", ADVANCE, &[], false)))])
    }
    const ASSET_PATH: &str = "/bots/dca_single_assets/pick_buyable_asset/new";

    #[test]
    fn the_first_screen_switches_the_order_from_the_track_and_has_no_search() {
        let v2 = || v2_page(&v2_step("Pick exchange", 25, &invite_sentence(), ""));
        let plain_track = || track(&[item("current", "Exchange", None), item("upcoming", "Assets", Some(("post", ORDER, &[("flow", "asset_first")], false))), item("upcoming", "Currency", None)]);
        let (mut theirs, mut ours) = (v2(), page(&sheet("modal_content", &plain_track(), &ours_step("", ""))));
        assert_eq!(wizard_sheet(EXCHANGES, None, &mut theirs, &mut ours), Ok(1));
        assert!(same(&theirs, &ours), "{theirs:#}\n{ours:#}");
        assert!(!theirs.to_string().contains("process-progress") && !theirs.to_string().contains("conversational"));
        // 2.x had no search on this screen.
        let with_search = ours_step(&search("exchange-picker", EXCHANGES, "exchange"), "");
        assert!(wizard_sheet(EXCHANGES, None, &mut v2(), &mut page(&sheet("modal_content", &plain_track(), &with_search))).is_err());
        // The switch with the other flow, or as a GET, is not the sentence's.
        for wrong in [item("upcoming", "Assets", Some(("post", ORDER, &[("flow", "exchange_first")], false))), item("upcoming", "Assets", Some(("get", ORDER, &[], false))), item("upcoming", "Assets", None)] {
            let drawn = track(&[item("current", "Exchange", None), wrong, item("upcoming", "Currency", None)]);
            assert!(wizard_sheet(EXCHANGES, None, &mut v2(), &mut page(&sheet("modal_content", &drawn, &ours_step("", "")))).is_err());
        }
    }

    #[test]
    fn the_asset_step_shows_the_tickers_opens_the_basket_and_mirrors_each_slot() {
        let v2 = |hidden: bool| v2_page(&v2_step("Pick asset", 75, &asset_sentence(), &basket(hidden)));
        let ours = |track: String, extras: String, hidden: bool| page(&sheet("modal_content", &track, &ours_step(&extras, &if hidden { basket(true) } else { ours_basket(true) })));
        for hidden in [true, false] {
            let (mut theirs, mut drawn) = (v2(hidden), ours(asset_track(&[("to", "exchange")]), next(""), false));
            assert_eq!(wizard_sheet(ASSET_PATH, Some("exchange_first"), &mut theirs, &mut drawn), Ok(1), "basket hidden in 2.x: {hidden}");
            assert!(same(&theirs, &drawn), "{theirs:#}\n{drawn:#}");
        }
        let refused = |track: String, extras: String, hidden: bool| wizard_sheet(ASSET_PATH, Some("exchange_first"), &mut v2(true), &mut ours(track, extras, hidden));
        assert!(refused(asset_track(&[("to", "exchange")]), next(""), true).is_err(), "a hidden basket");
        assert!(wizard_sheet(ASSET_PATH, Some("exchange_first"), &mut v2(true), &mut page(&sheet("modal_content", &asset_track(&[("to", "exchange")]), &ours_step(&next(""), &ours_basket(false))))).is_err(), "a basket row without its logo");
        assert!(refused(asset_track(&[("to", "exchange")]), next("").replace("<h3 class=\"label\">Add more assets</h3>", ""), false).is_err(), "Next without its label");
        assert!(refused(asset_track(&[("to", "assets")]), next(""), false).is_err(), "a changed `to`");
        assert!(refused(asset_track(&[]), next(""), false).is_err(), "the back without `to`");
        assert!(refused(asset_track(&[("to", "exchange")]), next("<input type=\"hidden\" name=\"to\" value=\"exchange\" />"), false).is_err(), "Next with `to`");
        assert!(refused(asset_track(&[("to", "exchange")]), format!("{}{}", next(""), next("")), false).is_err(), "two Next");
        assert!(refused(asset_track(&[("to", "exchange")]), format!("{}{}", next(""), search("asset-picker", "/q", "USD")), false).is_err(), "a search 2.x had not");
        let unnamed = track(&[item("done", "Exchange", Some(("post", ADVANCE, &[("to", "exchange")], true))), item("current", "BTC</span><span>ETH", None), item("upcoming", "Currency", Some(("post", ADVANCE, &[], false)))]);
        assert!(refused(unnamed, next(""), false).is_err(), "the venue's name not shown");
        let no_tickers = track(&[item("done", "Kraken", Some(("post", ADVANCE, &[("to", "exchange")], true))), item("current", "Assets", None), item("upcoming", "Currency", Some(("post", ADVANCE, &[], false)))]);
        assert!(refused(no_tickers, next(""), false).is_err(), "the tickers not shown");
        let two = format!("{}{}", asset_track(&[("to", "exchange")]), asset_track(&[("to", "exchange")]));
        assert!(refused(two, next(""), false).is_err(), "two tracks");
        // A changed field inside the step passes the adapter and is then a difference.
        let (mut theirs, mut drawn) = (v2(false), page(&sheet("modal_content", &asset_track(&[("to", "exchange")]), &ours_step(&next(""), &ours_basket(true)).replace("value=\"1\"", "value=\"2\""))));
        assert_eq!(wizard_sheet(ASSET_PATH, Some("exchange_first"), &mut theirs, &mut drawn), Ok(1));
        assert!(!same(&theirs, &drawn));
        assert!(wizard_sheet(ASSET_PATH, None, &mut v2(false), &mut page(&sheet("modal_frame", &asset_track(&[("to", "exchange")]), &ours_step(&next(""), &ours_basket(true))))).is_err(), "a wrong frame id");
    }

    #[test]
    fn the_key_steps_stream_carries_the_track_and_its_current_item_goes_back() {
        let stream = |inner: &str| json!(html::normalize(&format!("<turbo-stream action=\"update\" target=\"modal_content\"><template>{inner}\n</template></turbo-stream>\n<turbo-stream action=\"prepend\" target=\"flash\"><template><div>no</div></template></turbo-stream>")));
        let key_track = |exchange: Act| track(&[item("current", "Kraken", exchange), item("upcoming", "Assets", Some(("post", ORDER, &[("flow", "asset_first")], false))), item("upcoming", "Currency", None)]);
        let frame = |track: String| include_str!("../../templates/wizard/_sheet_frame.html").replace("{{ track|safe }}", &track).replace("{{ content|safe }}", &ours_step("", ""));
        let v2 = || stream(&v2_step("Connect exchange", 50, &key_sentence(), ""));
        let (mut theirs, mut ours) = (v2(), stream(&frame(key_track(Some(("get", EXCHANGES, &[], true))))));
        assert_eq!(wizard_sheet("/bots/dca_single_assets/add_api_key", None, &mut theirs, &mut ours), Ok(1));
        assert!(same(&theirs, &ours), "{theirs:#}\n{ours:#}");
        assert!(wizard_sheet("/bots/dca_single_assets/add_api_key", None, &mut v2(), &mut stream(&frame(key_track(None)))).is_err(), "the current venue must still go back");
        assert!(wizard_sheet("/bots/dca_single_assets/add_api_key", None, &mut v2(), &mut stream(&frame(key_track(Some(("get", EXCHANGES, &[], false)))))).is_err(), "the direction");
        assert!(wizard_sheet("/bots/dca_single_assets/add_api_key", None, &mut v2(), &mut stream(&ours_step("", ""))).is_err(), "no track");
    }

    #[test]
    fn a_search_answer_is_compared_inside_the_step() {
        let results = |r: &str| format!("<turbo-frame id=\"asset-picker\"><button>{r}</button></turbo-frame>");
        let v2 = || v2_page(&v2_step("Pick asset", 75, &asset_sentence(), &results("NVDA")));
        let ours = |r: &str| page(&sheet("modal_content", &asset_track(&[("to", "exchange")]), &ours_step(&next(""), &results(r))));
        let (mut theirs, mut drawn) = (v2(), ours("NVDA"));
        assert_eq!(wizard_sheet("/bots/dca_single_assets/pick_buyable_asset/new?query=nv", Some("exchange_first"), &mut theirs, &mut drawn), Ok(1));
        assert!(same(&theirs, &drawn));
        let (mut theirs, mut drawn) = (v2(), ours("NVDL"));
        assert_eq!(wizard_sheet("/bots/dca_single_assets/pick_buyable_asset/new?query=nv", Some("exchange_first"), &mut theirs, &mut drawn), Ok(1));
        assert!(!same(&theirs, &drawn), "a different result list");
    }

    #[test]
    fn the_exchange_step_groups_brokers_and_exchanges_in_v2_order() {
        let tile = |id: &str| format!("<div class=\"exchange-grid__option\"><button name=\"p\" type=\"submit\" value=\"{id}\" class=\"exchange-grid__item\" data-turbo-frame=\"modal_content\">{id}</button></div>");
        let v2_grid = format!("<div class=\"exchange-grid\">{}{}{}</div>", tile("1"), tile("14"), tile("3"));
        let group = |title: &str, ids: &[&str]| format!("<section class=\"exchange-group\"><h3 class=\"label\">{title}</h3><div class=\"exchange-grid\">{}</div></section>", ids.iter().map(|id| tile(id)).collect::<String>());
        let groups = |a: String, b: String| format!("<div class=\"exchange-groups\">{a}{b}</div>");
        let v2 = || v2_page(&v2_step("Pick exchange", 25, &invite_sentence(), &v2_grid));
        let plain_track = track(&[item("current", "Exchange", None), item("upcoming", "Assets", Some(("post", ORDER, &[("flow", "asset_first")], false))), item("upcoming", "Currency", None)]);
        let ours = |inner: String| page(&sheet("modal_content", &plain_track, &ours_step("", &inner)));
        let (mut theirs, mut drawn) = (v2(), ours(groups(group("Brokers", &["14"]), group("Exchanges", &["1", "3"]))));
        assert_eq!(wizard_sheet(EXCHANGES, None, &mut theirs, &mut drawn), Ok(1));
        assert!(same(&theirs, &drawn), "{theirs:#}\n{drawn:#}");
        // No venue: no group, 2.x's grid empty.
        let (mut theirs, mut drawn) = (v2_page(&v2_step("Pick exchange", 25, &invite_sentence(), "<div class=\"exchange-grid\"></div>")), ours(groups(String::new(), String::new())));
        assert_eq!(wizard_sheet(EXCHANGES, None, &mut theirs, &mut drawn), Ok(1));
        assert!(same(&theirs, &drawn));
        assert!(wizard_sheet(EXCHANGES, None, &mut v2(), &mut ours(groups(String::new(), String::new()))).is_err(), "2.x's tiles missing");
        // D-WIZARD-CONNECTED-GROUP: a first "Connected" group repeating venues below it is set aside.
        let connected = |ids: &[&str]| group("Connected", ids).replacen("class=\"exchange-group\"", "class=\"exchange-group exchange-group--connected\"", 1);
        let with = |first: String, rest: String| format!("<div class=\"exchange-groups\">{first}{rest}</div>");
        let rest = || format!("{}{}", group("Brokers", &["14"]), group("Exchanges", &["1", "3"]));
        let (mut theirs, mut drawn) = (v2(), ours(with(connected(&["3", "14"]), rest())));
        assert_eq!(wizard_sheet(EXCHANGES, None, &mut theirs, &mut drawn), Ok(1));
        assert!(same(&theirs, &drawn), "{theirs:#}\n{drawn:#}");
        // D-VENUE-TILE-ASKS-KEY: a Connected tile posts its account, and the form says `untargeted` among the groups.
        let account = |ids: &[&str]| connected(ids).replace("value=\"3\"", "value=\"3\" formaction=\"/p?account_id=7\"");
        let marked = |first: String, rest: String| format!("<div class=\"exchange-groups\"><input type=\"hidden\" name=\"untargeted\" value=\"1\" />{first}{rest}</div>");
        let (mut theirs, mut drawn) = (v2(), ours(marked(account(&["3", "14"]), rest())));
        assert_eq!(wizard_sheet(EXCHANGES, None, &mut theirs, &mut drawn), Ok(1));
        assert!(same(&theirs, &drawn), "{theirs:#}\n{drawn:#}");
        assert!(wizard_sheet(EXCHANGES, None, &mut v2(), &mut ours(marked(account(&["3"]).replace("class=\"exchange-grid__item\"", "class=\"other\""), rest()))).is_err(), "a Connected tile otherwise unlike its venue's");
        for wrong in [with(connected(&["9"]), rest()), with(connected(&[]), rest()), with(rest(), connected(&["3"])),
                      with(connected(&["3"]).replace("Connected", "Linked"), rest()), with(connected(&["3"]), group("Exchanges", &["1", "3"]))] {
            assert!(wizard_sheet(EXCHANGES, None, &mut v2(), &mut ours(wrong.clone())).is_err(), "{wrong}");
        }
        for wrong in [groups(group("Brokers", &["14"]), group("Exchanges", &["3", "1"])), groups(group("Brokers", &["14"]), group("Exchanges", &["1"])),
                      groups(group("Exchanges", &["14"]), group("Brokers", &["1", "3"])), groups(group("Brokers", &["14"]), group("Exchanges", &["1", "3", "9"]))] {
            assert!(wizard_sheet(EXCHANGES, None, &mut v2(), &mut ours(wrong.clone())).is_err(), "{wrong}");
        }
    }

    #[test]
    fn the_key_form_is_compared_by_what_it_submits() {
        let form = |fields: &str| format!("<form class=\"set-api__form form\" data-turbo-frame=\"modal_content\" action=\"/bots/dca_single_assets/add_api_key\" method=\"post\">{CSRF}{fields}</form><section class=\"set-api__instructions\">how</section>");
        let submit = |class: &str, value: &str| format!("<input type=\"submit\" name=\"commit\" value=\"{value}\" class=\"{class}\" data-turbo-stream=\"true\" data-turbo-submits-with=\"Validating\" data-disable-with=\"{value}\" />");
        let v2_fields = format!("<div class=\"form__row form__row--input\"><input autofocus=\"autofocus\" autocomplete=\"off\" data-controller=\"autofocus\" class=\"form__input\" type=\"text\" name=\"api_key[key]\" id=\"api_key_key\" /><label class=\"form__label\" for=\"api_key_key\">API Key</label></div><div class=\"form__row form__row--input\"><input autocomplete=\"off\" class=\"form__input\" type=\"text\" name=\"api_key[secret]\" id=\"api_key_secret\" /><label class=\"form__label\" for=\"api_key_secret\">Secret Key</label></div><div class=\"set-api__actions\">{}</div>", submit("button button--accent", "Connect"));
        let ours_fields = |key_name: &str, value: &str| format!("<input autofocus=\"autofocus\" autocomplete=\"off\" data-controller=\"autofocus\" class=\"rinput rinput--outline rinput--text\" placeholder=\"API Key\" aria-label=\"API Key\" type=\"text\" name=\"{key_name}\" id=\"api_key_key\" /><input autocomplete=\"off\" class=\"rinput rinput--outline rinput--text\" placeholder=\"Secret Key\" aria-label=\"Secret Key\" type=\"text\" name=\"api_key[secret]\" id=\"api_key_secret\" /><div class=\"set-api__actions\">{}</div>", submit("rbutton rbutton--secondary", value));
        let key_track = track(&[item("current", "Kraken", Some(("get", EXCHANGES, &[], true))), item("upcoming", "Assets", Some(("post", ORDER, &[("flow", "asset_first")], false))), item("upcoming", "Currency", None)]);
        let v2 = || v2_page(&v2_step("Connect exchange", 50, &key_sentence(), &form(&v2_fields)));
        let ours = |fields: String| page(&sheet("modal_content", &key_track, &ours_step("", &form(&fields))));
        let (mut theirs, mut drawn) = (v2(), ours(ours_fields("api_key[key]", "Connect")));
        assert_eq!(wizard_sheet("/bots/dca_single_assets/add_api_key/new", None, &mut theirs, &mut drawn), Ok(1));
        assert!(same(&theirs, &drawn), "{theirs:#}\n{drawn:#}");
        assert!(wizard_sheet("/bots/dca_single_assets/add_api_key/new", None, &mut v2(), &mut ours(ours_fields("api_key[public]", "Connect"))).is_err(), "a renamed field");
        assert!(wizard_sheet("/bots/dca_single_assets/add_api_key/new", None, &mut v2(), &mut ours(ours_fields("api_key[key]", "Save"))).is_err(), "another button");
        assert!(wizard_sheet("/bots/dca_single_assets/add_api_key/new", None, &mut v2(), &mut ours(ours_fields("api_key[key]", "Connect").replace("aria-label=\"Secret Key\"", "aria-label=\"Password\""))).is_err(), "another name");
    }

    #[test]
    fn a_refused_key_is_said_under_the_fields_as_v2_flashed_it() {
        let header = |title: &str| format!("<header class=\"set-api__brand\"><span class=\"set-api__mark\">m</span><h2 class=\"set-api__title\">{title}</h2></header>");
        let submit = |class: &str| format!("<input type=\"submit\" name=\"commit\" value=\"Connect\" class=\"{class}\" data-turbo-stream=\"true\" />");
        let v2_card = format!("<div class=\"set-api set-api--connect\">{}<form class=\"set-api__form form\" action=\"/k\" method=\"post\">{CSRF}<div class=\"form__row\"><input class=\"form__input\" type=\"text\" name=\"api_key[key]\" id=\"api_key_key\" /><label class=\"form__label\" for=\"api_key_key\">API Key</label></div><div class=\"set-api__actions\">{}</div></form></div>",
                                 header("Connect Binance"), submit("button button--accent"));
        let ours_card = |title: &str, reason: &str| format!("<div class=\"set-api set-api--connect\">{}<form class=\"set-api__form form\" action=\"/k\" method=\"post\">{CSRF}<input class=\"rinput rinput--outline rinput--text rinput--invalid\" aria-label=\"API Key\" aria-invalid=\"true\" type=\"text\" name=\"api_key[key]\" id=\"api_key_key\" /><p class=\"sform__error\" id=\"api_key_error\" role=\"alert\">{reason}</p><div class=\"set-api__actions\">{}</div></form></div>",
                                 header(title), submit("rbutton rbutton--secondary"));
        let flash = |reason: &str| format!("\n<turbo-stream action=\"prepend\" target=\"flash\"><template><div class=\"flash__message\" role=\"alert\">{reason}</div></template></turbo-stream>");
        let update = |inner: &str| format!("<turbo-stream action=\"update\" target=\"modal_content\"><template>{inner}\n</template></turbo-stream>");
        let key_track = track(&[item("current", "Binance", Some(("get", EXCHANGES, &[], true))), item("upcoming", "Assets", Some(("post", ORDER, &[("flow", "asset_first")], false))), item("upcoming", "Currency", None)]);
        let frame = |card: String| include_str!("../../templates/wizard/_sheet_frame.html").replace("{{ track|safe }}", &key_track).replace("{{ content|safe }}", &ours_step("", &card));
        let v2 = |reason: &str| json!(html::normalize(&format!("{}{}", update(&v2_step("Connect exchange", 50, &key_sentence().replace("Kraken", "Binance"), &v2_card)), flash(reason))));
        let ours = |title: &str, reason: &str| json!(html::normalize(&update(&frame(ours_card(title, reason)))));
        let path = "/bots/dca_single_assets/add_api_key";
        let (mut theirs, mut drawn) = (v2("Incorrect API key permissions"), ours("Binance", "Incorrect API key permissions"));
        assert_eq!(wizard_sheet(path, None, &mut theirs, &mut drawn), Ok(1));
        assert!(same(&theirs, &drawn), "{theirs:#}\n{drawn:#}");
        assert!(wizard_sheet(path, None, &mut v2("Incorrect API key permissions"), &mut ours("Binance", "Something else")).is_err(), "another reason");
        assert!(wizard_sheet(path, None, &mut json!(html::normalize(&update(&v2_step("Connect exchange", 50, &key_sentence(), &v2_card)))), &mut ours("Binance", "Incorrect API key permissions")).is_err(), "2.x flashed nothing");
        assert!(wizard_sheet(path, None, &mut v2("Incorrect API key permissions"), &mut ours("Kraken", "Incorrect API key permissions")).is_err(), "another venue's name");
    }

    #[test]
    fn the_broker_step_has_a_plain_track() {
        let v2 = || v2_page(&format!("<div class=\"bot-creation-layout\">\n    {}\n\n    {}\n  </div>\n", bar("Pick exchange", 50), body("")));
        let plain = track(&[item("done", "BTC", None), item("current", "Exchange", None), item("upcoming", "Currency", None)]);
        let (mut theirs, mut ours) = (v2(), page(&sheet("modal_content", &plain, &ours_step("", ""))));
        assert_eq!(wizard_sheet("/bots/dca_multi_assets/pick_stock_broker/new", Some("asset_first"), &mut theirs, &mut ours), Ok(1));
        assert!(same(&theirs, &ours));
        let linked = track(&[item("done", "BTC", Some(("get", ASSETS, &[], true))), item("current", "Exchange", None), item("upcoming", "Currency", None)]);
        assert!(wizard_sheet("/bots/dca_multi_assets/pick_stock_broker/new", Some("asset_first"), &mut v2(), &mut page(&sheet("modal_content", &linked, &ours_step("", "")))).is_err(),
                "2.x offered no way back there");
    }

    #[test]
    fn index_signal_lazy_pages_and_refusals_are_not_touched() {
        for path in ["/bots/dca_indexes/pick_exchange/new", "/bots/signals/pick_buyable_asset/new", "/de/bots/dca_indexes/add_api_key"] {
            let (mut theirs, mut ours) = (v2_page(&v2_step("Pick exchange", 25, &invite_sentence(), "")), page("<div>other</div>"));
            let before = ours.clone();
            assert_eq!(wizard_sheet(path, None, &mut theirs, &mut ours), Ok(0), "{path}");
            assert_eq!(ours, before);
        }
        let lazy = json!(html::normalize("<turbo-frame id=\"assets-page-20\"><button>NVDA</button></turbo-frame>"));
        let (mut theirs, mut ours) = (lazy.clone(), lazy.clone());
        assert_eq!(wizard_sheet("/bots/dca_single_assets/pick_buyable_asset/new?offset=20", None, &mut theirs, &mut ours), Ok(0));
        assert_eq!((theirs, ours), (lazy.clone(), lazy));
        // This build refuses the step (a venue it does not trade): the comparison holds the refusal as before.
        let refusal = page("<div class=\"not-ported\">no</div>");
        let (mut theirs, mut ours) = (v2_page(&v2_step("Connect exchange", 50, &key_sentence(), "")), refusal.clone());
        assert_eq!(wizard_sheet("/bots/dca_single_assets/add_api_key/new", None, &mut theirs, &mut ours), Ok(0));
        assert_eq!(ours, refusal);
    }

    /// The Asset / Index switch as this crate draws it in the asset step's search row.
    fn switch(prefix: &str, asset: &str, index: &str, frame: &str, current_on_index: bool) -> String {
        let link = |kind: &str, label: &str, current: bool| format!("<a class=\"segmented__option{}\"{} data-value=\"{kind}\" data-segmented-target=\"option\" data-action=\"segmented#select\" data-turbo-frame=\"{frame}\" href=\"{prefix}{ASSET_STEP}?kind={kind}\">{label}</a>",
            if current { " is-on" } else { "" }, if current { " aria-current=\"page\"" } else { "" });
        format!("<div class=\"segmented segmented--search\" aria-label=\"Kind\" data-controller=\"segmented\" data-action=\"click-&gt;segmented#toggle\"><span class=\"segmented__thumb\" data-segmented-target=\"thumb\"></span><div class=\"segmented__dropdown dropdown\" data-segmented-target=\"dropdown\"><div class=\"segmented__panes\"><div class=\"segmented__pane segmented__menu\">{}{}</div></div></div><span class=\"segmented__caret\" aria-hidden=\"true\"><svg class=\"icon-16\"><path d=\"m\"></path></svg></span></div>",
                link("asset", asset, !current_on_index), link("index", index, current_on_index))
    }
    /// The asset step's block: the basket, then 2.x's search row (its zoom icon) or this crate's (`row`), or the cap's line.
    fn assets_block(row: &str) -> String { format!("<div class=\"wizard-assets vt-step-body\">{}{row}</div>", basket(false)) }
    /// This crate's: the search first (D-ASSETS-SEARCH-FIRST), then the basket and the rest.
    fn ours_block(search: &str, rest: &str) -> String { format!("<div class=\"wizard-assets vt-step-body\">{search}{}{rest}</div>", basket(false)) }
    fn v2_row() -> String { "<div class=\"modal--search\"><form data-turbo-frame=\"asset-picker\" style=\"position:relative\" action=\"/de/bots/dca_single_assets/pick_buyable_asset/new\" method=\"get\"><svg class=\"icon-24\"><path d=\"zoom\"></path></svg><input name=\"query\" type=\"text\" /></form></div>".into() }
    fn ours_row(switch: &str, kind: &str) -> String {
        format!("<div class=\"wizard-search\"><form class=\"wizard-search__form\" data-turbo-frame=\"asset-picker\" action=\"/de/bots/dca_single_assets/pick_buyable_asset/new\" method=\"get\">{switch}{kind}<input name=\"query\" type=\"text\" /></form></div>")
    }
    const KIND: &str = "<input type=\"hidden\" name=\"kind\" value=\"asset\" />";
    const CAP: &str = "<p class=\"text-inactive text-center\">You can add up to 100 assets.</p>";
    const DE_ASSETS: &str = "/de/bots/dca_single_assets/pick_buyable_asset/new";

    #[test]
    fn the_asset_steps_search_row_holds_the_asset_index_switch_in_place_of_the_icon() {
        // Exchange first on the asset step with nothing chosen: the venue goes back by the advance, nothing else.
        let sentence = format!("<div class=\"conversational conversational--bot-setup\"><span class=\"conversational__lead\">Buy</span><div class=\"conversational__assets\"></div><span class=\"conversational__lead\">on</span>{}</div>",
                               post("conversational__link filled", ADVANCE, &[("to", "exchange")], true, "value=\"Kraken\""));
        let v2 = |block: &str| v2_page(&v2_step("Pick asset", 75, &sentence, block));
        let drawn = track(&[item("done", "Kraken", Some(("post", ADVANCE, &[("to", "exchange")], true))), item("current", "Assets", None), item("upcoming", "Currency", None)]);
        let ours = |block: &str| page(&sheet("modal_content", &drawn, &ours_step("", block)));
        let good = || switch("/de", "Asset", "Index", "modal_content", false);
        let (mut theirs, mut mine) = (v2(&assets_block(&v2_row())), ours(&ours_block(&ours_row(&good(), KIND), "")));
        assert_eq!(wizard_sheet(DE_ASSETS, Some("exchange_first"), &mut theirs, &mut mine), Ok(1));
        assert!(same(&theirs, &mine), "{theirs:#}\n{mine:#}");
        // At the cap 2.x draws no row; this crate's is the switch alone.
        let (mut theirs, mut mine) = (v2(&assets_block(CAP)), ours(&ours_block(&format!("<div class=\"wizard-search\">{}</div>", good()), CAP)));
        assert_eq!(wizard_sheet(DE_ASSETS, Some("exchange_first"), &mut theirs, &mut mine), Ok(1));
        assert!(same(&theirs, &mine), "{theirs:#}\n{mine:#}");
        let refused = |block: String| wizard_sheet(DE_ASSETS, Some("exchange_first"), &mut v2(&assets_block(&v2_row())), &mut ours(&block));
        for (wrong, why) in [(switch("", "Asset", "Index", "modal_content", false), "no locale prefix"), (switch("/de", "Asset", "Index", "modal", false), "the modal frame"),
                             (switch("/de", "Asset", "Index", "modal_content", true), "Index current"), (switch("/de", "Assets", "Index", "modal_content", false), "another label"),
                             (switch("/de", "Asset", "Index", "modal_content", false).replace("kind=index", "kind=indices"), "a wrong href"),
                             (switch("/de", "Asset", "Index", "modal_content", false).replace("data-controller=", "data-segmented-frame=\"modal_content\" data-controller="), "page history")] {
            assert!(refused(ours_block(&ours_row(&wrong, KIND), "")).is_err(), "{why}");
        }
        assert!(refused(ours_block(&ours_row("", KIND), "")).is_err(), "no switch");
        assert!(refused(ours_block(&ours_row(&format!("{}{}", good(), good()), KIND), "")).is_err(), "two switches");
        assert!(refused(ours_block(&ours_row(&good(), ""), "")).is_err(), "the search form without kind=asset");
        assert!(refused(ours_block(&ours_row(&good(), &KIND.replace("asset", "index")), "")).is_err(), "kind=index on the Asset view");
        assert!(wizard_sheet(DE_ASSETS, Some("exchange_first"), &mut v2(&assets_block(CAP)), &mut ours(&ours_block(&ours_row(&good(), KIND), CAP))).is_err(),
                "a search box at the cap");
        // Off the asset step there is no switch.
        let v2_exchange = || v2_page(&v2_step("Pick exchange", 25, &invite_sentence(), ""));
        let plain = track(&[item("current", "Exchange", None), item("upcoming", "Assets", Some(("post", ORDER, &[("flow", "asset_first")], false))), item("upcoming", "Currency", None)]);
        assert!(wizard_sheet(EXCHANGES, None, &mut v2_exchange(), &mut page(&sheet("modal_content", &plain, &ours_step("", &good())))).is_err(), "a switch on the exchange step");
    }

    fn answer(status: u16, location: Option<&str>, body: &str, session: Option<Value>) -> Value {
        let mut answer = json!({"status": status, "headers": {}, "body": html::normalize(body)});
        if let Some(location) = location { answer["headers"]["location"] = json!(location); }
        if let Some(session) = session { answer["session"] = session; }
        answer
    }
    fn tiles(dca: &str, index: bool, signal: bool) -> String {
        format!("<div class=\"itiles\"><a class=\"itile itile--bot-type\" {dca} href=\"/de/bots/dca_single_assets/pick_exchange/new\">DCA</a>{}{}</div>",
                if index { "<a class=\"itile itile--bot-type\" data-turbo-frame=\"modal_content\" data-wizard-dir=\"enter\" href=\"/de/bots/dca_indexes/setup_coingecko/new\">Index</a>" } else { "" },
                if signal { "<a class=\"itile itile--bot-type\" data-turbo-frame=\"modal_content\" data-wizard-dir=\"enter\" href=\"/de/bots/signals/pick_exchange/new\">Signal</a>" } else { "" })
    }
    const V2_DCA: &str = "data-turbo-frame=\"modal_content\" data-wizard-dir=\"enter\"";

    #[test]
    fn the_chooser_has_no_index_tile_and_without_advanced_bots_is_skipped() {
        let session = || Some(json!({"bot_config": {}}));
        // Advanced bots: 2.x's Index tile left out, this crate's DCA tile read back as 2.x's.
        let (mut theirs, mut ours) = (answer(200, None, &tiles(V2_DCA, true, true), session()), answer(200, None, &tiles("data-turbo-frame=\"modal\"", false, true), session()));
        assert_eq!(chooser("/de/bots/new", &mut theirs, &mut ours), Ok(1));
        assert_eq!(theirs["body"], answer(200, None, &tiles(V2_DCA, false, true), None)["body"]);
        assert_eq!(theirs["body"], ours["body"]);
        assert!(chooser("/de/bots/new", &mut answer(200, None, &tiles(V2_DCA, true, true), None), &mut answer(200, None, &tiles("data-turbo-frame=\"modal\"", true, true), None)).is_err(), "an Index tile kept");
        assert!(chooser("/de/bots/new", &mut answer(200, None, &tiles(V2_DCA, true, true), None), &mut answer(200, None, &tiles(V2_DCA, false, true), None)).is_err(), "the old DCA tile");
        assert!(chooser("/de/bots/new", &mut answer(200, None, &tiles(V2_DCA, true, false), None), &mut answer(200, None, &tiles("data-turbo-frame=\"modal\"", false, false), None)).is_err(),
                "a chooser drawn for a user without advanced bots");
        // No advanced bots: the 302 to the exchange step, the wizard begun.
        let skipped = |location: &str, session: Option<Value>| answer(302, Some(location), "", session);
        let (mut theirs, mut ours) = (answer(200, None, &tiles(V2_DCA, true, false), session()), skipped("/de/bots/dca_single_assets/pick_exchange/new", session()));
        assert_eq!(chooser("/de/bots/new", &mut theirs, &mut ours), Ok(1));
        assert_eq!(theirs, ours);
        for (location, session, why) in [("/de/bots/dca_single_assets/pick_buyable_asset/new", session(), "another step"),
                                         ("/de/bots/dca_single_assets/pick_exchange/new", Some(json!({"bot_config": null})), "the wizard not begun")] {
            assert!(chooser("/de/bots/new", &mut answer(200, None, &tiles(V2_DCA, true, false), Some(json!({"bot_config": {}}))), &mut skipped(location, session)).is_err(), "{why}");
        }
        assert!(chooser("/de/bots/new", &mut answer(200, None, &tiles(V2_DCA, true, true), None), &mut skipped("/de/bots/dca_single_assets/pick_exchange/new", None)).is_err(),
                "skipped where 2.x offered Signal");
        // Signed out, and other paths: not touched.
        let (mut theirs, mut ours) = (answer(302, Some("/login"), "", None), answer(302, Some("/login"), "", None));
        assert_eq!(chooser("/bots/new", &mut theirs, &mut ours), Ok(0));
        assert_eq!(chooser("/bots", &mut answer(200, None, &tiles(V2_DCA, true, false), None), &mut answer(200, None, "<p>x</p>", None)), Ok(0));
    }
    /// D-SHEET-EVERYWHERE's hook: a sheet step exempted only when its shell, close URL, POST actions and venues hold.
    mod key_cards_hook {
        use super::*;
        const BOT: &str = "/es/bots/1/add_api_key/new";
        fn v2_card(form: &str) -> String { format!("<turbo-frame id=\"modal\"><dialog class=\"dialog\"><div class=\"modal\">{form}</div></dialog></turbo-frame>") }
        fn sheet(close: &str, pane: &str) -> String {
            format!("<turbo-frame id=\"modal\"><dialog class=\"dialog sheet-dialog\" data-modal--base-close-url-value=\"{close}\"><div class=\"sheet\"><turbo-frame class=\"sheet__frame\" id=\"modal_content\"><div class=\"sheet__pane\">{pane}</div></turbo-frame></div></dialog></turbo-frame>")
        }
        fn form(action: &str, fields: &[&str]) -> String {
            let fields: String = fields.iter().map(|f| format!("<input type=\"text\" name=\"{f}\">")).collect();
            format!("<form action=\"{action}\" method=\"post\"><input type=\"hidden\" name=\"authenticity_token\" value=\"t\">{fields}<input type=\"submit\" name=\"commit\" value=\"Connect\"></form>")
        }
        const FIELDS: [&str; 3] = ["api_key[key]", "api_key[secret]", "api_key[passphrase]"];

        #[test]
        fn the_bot_key_card_is_exempted_only_with_2xs_form_contract() {
            let theirs = || page(&v2_card(&form("/es/bots/1/add_api_key", &FIELDS)));
            let ok = page(&sheet("/es/bots/1", &form("/es/bots/1/add_api_key", &FIELDS)));
            let (mut a, mut b) = (theirs(), ok);
            assert_eq!(key_cards(BOT, "GET", &Value::Null, &mut a, &mut b), Ok(1));
            assert_eq!(a, b, "both cards are one marker, the page around them compared");
            let fails = |ours: String| key_cards(BOT, "GET", &Value::Null, &mut theirs(), &mut page(&ours)).is_err();
            assert!(fails(sheet("/es/bots/1", &form("/es/bots/1/add_api_key", &FIELDS[..2]))), "a field missing");
            assert!(fails(sheet("/es/bots/1", &form("/es/bots/2/add_api_key", &FIELDS))), "another action");
            assert!(fails(sheet("/es/tracker", &form("/es/bots/1/add_api_key", &FIELDS))), "closing elsewhere");
            assert!(fails(sheet("/es/bots/1", &format!("<ol class=\"sheet__track\"></ol>{}", form("/es/bots/1/add_api_key", &FIELDS)))), "a track");
        }

        #[test]
        fn a_refused_card_is_one_stream_with_its_form_and_2xs_reason() {
            const POST: &str = "/bots/1/add_api_key";
            let theirs = || json!(html::normalize("<turbo-stream action=\"prepend\" target=\"flash\"><template><div class=\"salert\">The key is incorrect.</div></template></turbo-stream>"));
            let ours = |action: &str, reason: &str, extra: &str| json!(html::normalize(&format!("<turbo-stream action=\"update\" target=\"modal_content\"><template><div class=\"sheet__pane\">{}<p class=\"sform__error\" role=\"alert\">{reason}</p></div></template></turbo-stream>{extra}", form(action, &FIELDS))));
            // A passphrase that is no Paper/Live value (the stub form sends none either).
            let posted = json!({"api_key[key]": "k", "api_key[secret]": "s", "api_key[passphrase]": "p"});
            assert_eq!(key_cards(POST, "POST", &posted, &mut theirs(), &mut ours(POST, "The key is incorrect.", "")), Ok(1));
            assert!(key_cards(POST, "POST", &json!({"api_key[key]": "k", "api_key[secret]": "s", "api_key[passphrase]": "p", "api_key[extra]": "x"}), &mut theirs(), &mut ours(POST, "The key is incorrect.", "")).is_err(), "a posted field missing from the redraw");
            assert!(key_cards(POST, "POST", &posted, &mut theirs(), &mut ours(POST, "The key is", "")).is_err(), "a truncated reason");
            assert!(key_cards(POST, "POST", &posted, &mut theirs(), &mut ours("/bots/2/add_api_key", "The key is incorrect.", "")).is_err(), "another action");
            assert!(key_cards(POST, "POST", &posted, &mut theirs(), &mut ours(POST, "The key is incorrect.", "<turbo-stream action=\"prepend\" target=\"flash\"><template>x</template></turbo-stream>")).is_err(), "an extra stream");
            let no_form = json!(html::normalize("<turbo-stream action=\"update\" target=\"modal_content\"><template><p class=\"sform__error\">The key is incorrect.</p></template></turbo-stream>"));
            assert!(key_cards(POST, "POST", &posted, &mut theirs(), &mut no_form.clone()).is_err(), "no retry form");
            assert!(key_cards(POST, "POST", &json!({"api_key[key]": "k", "api_key[secret]": "s", "api_key[passphrase]": "live"}), &mut theirs(), &mut ours(POST, "The key is incorrect.", "")).is_err(), "the Paper/Live value lost");
        }
    }

    mod sheet_flows_hook {
        use super::*;
        fn dialog(close: &str, inner: &str) -> String {
            format!("<turbo-frame id=\"modal\"><dialog data-controller=\"modal--base\" data-modal--base-close-url-value=\"{close}\" class=\"dialogview\"><div class=\"dialogview__vertical-center\"><turbo-frame id=\"modal_content\"><div class=\"bot-creation-layout\">{inner}</div></turbo-frame></div></dialog></turbo-frame>")
        }
        fn sheet(close: &str, track: bool, inner: &str) -> String {
            let track = if track { "<ol class=\"sheet__track\"><li class=\"sheet__step sheet__step--current\"><span class=\"sheet__control\">Exchange</span></li></ol>" } else { "" };
            format!("<turbo-frame id=\"modal\"><dialog class=\"dialog sheet-dialog\" data-controller=\"modal--base\" data-modal--base-close-url-value=\"{close}\"><div class=\"sheet\"><turbo-frame class=\"sheet__frame\" id=\"modal_content\"><nav class=\"sheet__side\">{track}</nav><div class=\"sheet__pane sheet__pane--flush\"><div class=\"bot-creation-layout\">{inner}</div></div></turbo-frame></div></dialog></turbo-frame>")
        }
        fn venue(id: &str) -> String { format!("<button name=\"bots_dca_single_asset[exchange_id]\" type=\"submit\" value=\"{id}\">{id}</button>") }
        fn grid(ids: &[&str]) -> String { format!("<form action=\"/rules/withdrawals/pick_exchange\" method=\"post\">{}</form>", ids.iter().map(|id| venue(id)).collect::<String>()) }
        fn groups(ids: &[&str]) -> String { format!("<form action=\"/rules/withdrawals/pick_exchange\" method=\"post\"><section class=\"exchange-group\">{}</section></form>", ids.iter().map(|id| venue(id)).collect::<String>()) }
        const RULES: &str = "/rules/withdrawals/pick_exchange/new";

        #[test]
        fn a_sheet_step_is_exempted_once_its_shell_actions_and_venues_hold() {
            let (mut theirs, mut ours) = (page(&dialog("/rules", &grid(&["1", "5", "2"]))), page(&sheet("/rules", true, &groups(&["1", "2"]))));
            assert_eq!(sheet_flows(RULES, "GET", &mut theirs, &mut ours), Ok(1));
            assert_eq!(theirs, ours, "the step set aside, the page around it compared");
            let fails = |theirs: &str, ours: &str| sheet_flows(RULES, "GET", &mut page(theirs), &mut page(ours)).is_err();
            assert!(fails(&dialog("/rules", &grid(&["1", "5", "2"])), &sheet("/bots", true, &groups(&["1", "2"]))), "a wrong close URL");
            assert!(fails(&dialog("/rules", &grid(&["1", "5", "2"])), &sheet("/rules", false, &groups(&["1", "2"]))), "no track");
            assert!(fails(&dialog("/rules", &grid(&["1", "5", "2"])), &sheet("/rules", true, "<p>no form</p>")), "a dropped POST action");
            assert!(fails(&dialog("/rules", &grid(&["1", "5", "2"])), &sheet("/rules", true, &groups(&["1"]))), "a venue missing besides Binance US");
            assert!(fails(&dialog("/rules", &grid(&["1", "5", "2"])), &sheet("/rules", true, &groups(&["2", "1"]))), "a venue group out of 2.x's order");
            // D-WIZARD-CONNECTED-GROUP: a first Connected group repeats venues below it; one it alone holds, or one not first, fails.
            let connected = |ids: &[&str]| format!("<section class=\"exchange-group exchange-group--connected\">{}</section>", ids.iter().map(|id| venue(id)).collect::<String>());
            let rest = "<section class=\"exchange-group\"><button name=\"bots_dca_single_asset[exchange_id]\" type=\"submit\" value=\"1\">1</button><button name=\"bots_dca_single_asset[exchange_id]\" type=\"submit\" value=\"2\">2</button></section>";
            let form = |inner: String| format!("<form action=\"/rules/withdrawals/pick_exchange\" method=\"post\">{inner}</form>");
            assert_eq!(sheet_flows(RULES, "GET", &mut page(&dialog("/rules", &grid(&["1", "5", "2"]))), &mut page(&sheet("/rules", true, &form(format!("{}{rest}", connected(&["2"])))))), Ok(1));
            assert!(fails(&dialog("/rules", &grid(&["1", "5", "2"])), &sheet("/rules", true, &form(format!("{}{rest}", connected(&["5"]))))), "a connected venue not offered below");
            assert!(fails(&dialog("/rules", &grid(&["1", "5", "2"])), &sheet("/rules", true, &form(format!("{rest}{}", connected(&["2"]))))), "the Connected group not first");
            assert!(fails(&dialog("/rules", &grid(&["1", "2"])), &sheet("/rules", true, &groups(&["1", "2"]))), "2.x no longer offers Binance US");
        }

        #[test]
        fn a_refusal_in_the_sheet_must_say_what_2x_flashed() {
            let ours = |reason: &str| json!(html::normalize(&format!("<turbo-stream action=\"update\" target=\"modal_content\"><template><nav class=\"sheet__side\"></nav><div class=\"sheet__pane\"><p class=\"sform__error\" id=\"api_key_error\" role=\"alert\">{reason}</p></div></template></turbo-stream>")));
            let theirs = || json!(html::normalize("<turbo-stream action=\"prepend\" target=\"flash\"><template><div class=\"salert\">The key is incorrect.</div></template></turbo-stream>"));
            assert_eq!(sheet_flows("/tracker/add_api_key", "POST", &mut theirs(), &mut ours("The key is incorrect.")), Ok(1));
            assert!(sheet_flows("/tracker/add_api_key", "POST", &mut theirs(), &mut ours("Another reason.")).is_err());
        }

        #[test]
        fn the_start_button_differs_by_its_class_alone() {
            let button = |class: &str| json!(html::normalize(&format!("<turbo-stream action=\"replace\" target=\"create-bot-button\"><template><div id=\"create-bot-button\"><form action=\"/bots/signals\" method=\"post\"><input type=\"submit\" name=\"commit\" value=\"Start\" class=\"{class}\" /></form></div></template></turbo-stream>")));
            let (mut theirs, mut ours) = (button("sbutton button--success"), button("rbutton rbutton--primary"));
            assert_eq!(sheet_flows("/bots/signals/confirm_settings", "POST", &mut theirs, &mut ours), Ok(1));
            assert_eq!(theirs, ours);
            assert!(sheet_flows("/bots/signals/confirm_settings", "POST", &mut button("sbutton button--success"), &mut button("sbutton")).is_err());
        }
    }

}
