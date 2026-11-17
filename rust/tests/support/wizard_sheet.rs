//! D-WIZARD-SHEET, D-WIZARD-NO-SENTENCE and D-WIZARD-TRACK (named divergences from the 2.x recordings): a DCA wizard step
//! is the sheet Settings uses, its track in the side column and the step in the pane, where Rails draws it in the full-page
//! dialog with a progress bar and the conversational sentence on top. The step itself (the `.bot-creation-layout` subtree:
//! table, basket, search, form) is compared as it was, with Rails' progress bar and sentence left out, Rails' basket read as
//! open, and this crate's pane search box and Next button taken out — each required to be exactly the form Rails' sentence
//! had for it. Around the step, this crate's lines must be exactly the sheet (`wizard/sheet.html`, or for the key step's 422
//! stream `wizard/_sheet_frame.html`) with one track, and Rails' exactly the full-page shell. The track's three items are
//! checked against Rails' progress bar (which is current) and its sentence (each item's text, and the request its slot
//! made: method, URL, fields, frame, direction). Then this crate's shell is put back as Rails' around this crate's step,
//! so everything else is compared as before. On the chooser only the DCA tile's frame target and its dropped
//! `data-wizard-dir` are left out. The index and signal wizards, and a lazy page of results (no shell), are not touched.
use super::{attr, html, is_space, subtree_end};
use serde_json::{json, Value};

const LOCALES: [&str; 15] = ["en", "de", "fr", "it", "es", "pt", "nl", "pl", "ru", "bg", "el", "sv", "da", "cs", "sk"];
/// The track's items (exchange 0, assets 1, currency 2) in each order, and the item each of Rails' four progress steps is.
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

/// The full-page dialog Rails draws a step in (`wizard/full_page.html` writes it as Rails does, `complete` aside), the step as
/// a placeholder.
fn full_page_shell(close_url: &str) -> Vec<Value> {
    let page = include_str!("../../templates/wizard/full_page.html").replace("{{ close_url }}", &escape(close_url))
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

/// What Rails' sentence offered: per item (exchange, assets, currency) the request its slot made and the text it showed (the
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

/// Rails' progress bar in its step: the title and the share, 25 to 100.
fn progress(lines: &[Value], from: usize, to: usize) -> Result<(usize, String, usize), String> {
    let bar = positions(lines, from, to, |l| has_class(l, "div", "process-progress")).first().copied().ok_or("Rails' step has no progress bar")?;
    let end = subtree_end(lines, bar);
    let title = (bar..end).find(|i| opens(&lines[*i], "h4")).and_then(|h| words(&lines[h + 1])).ok_or("no title")?;
    let share = (bar..end).find(|i| has_class(&lines[*i], "div", "slider__style__progress")).and_then(|i| attr(text(&lines[i]), "style"))
        .and_then(|style| style.strip_prefix("width: ")?.strip_suffix('%')?.parse().ok()).ok_or("no share")?;
    Ok((bar, title, share))
}

/// The orders and locales the step may be in, by Rails' progress bar (its title names the step, its share the place):
/// (locale, the items in their order, the current item's place).
/// A reading of Rails' progress bar: the locale, the items in their order, the current item's place.
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
fn cut(lines: &mut Vec<Value>, at: usize) {
    let end = subtree_end(lines, at);
    let start = if at > 0 && is_space(&lines[at - 1]) { at - 1 } else { at };
    lines.drain(start..end);
}

/// One item as Rails' step implies it: its state, its text (None: not compared) and its request.
type Expected<'a> = (String, Option<Vec<String>>, Option<&'a Request>);
/// One item of this crate's track: its state, its text and its request.
type Drawn = (String, Vec<String>, Option<Request>);

/// The track's checks against Rails' step: one reading of its progress bar (locale, order, current item) under which every
/// item has its state by position, its text (Rails' sentence's, else the item's label) and exactly its slot's request.
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
/// Rails draws one. Each group must keep Rails' order and the two must hold exactly Rails' tiles, under the locale's headings
/// (a group with no venue is not drawn); then this crate's groups are put back as Rails' grid.
fn ungroup(theirs: &[Value], step: usize, ours: &mut Vec<Value>, ours_step: usize, locale: &str) -> Result<(), String> {
    let Some(groups) = positions(ours, ours_step, subtree_end(ours, ours_step), |l| has_class(l, "div", "exchange-groups")).first().copied() else { return Ok(()) };
    let grid = positions(theirs, step, subtree_end(theirs, step), |l| has_class(l, "div", "exchange-grid")).first().copied().ok_or("venue groups where Rails has no grid")?;
    let options = |lines: &[Value], at: usize| -> Vec<Vec<Value>> {
        children(lines, at, subtree_end(lines, at), depth(&lines[at])).into_iter().map(|o| without_space(&reindent(&lines[o..subtree_end(lines, o)], 0))).collect()
    };
    let rails = options(theirs, grid);
    let groups_end = subtree_end(ours, groups);
    let (mut drawn, mut titles) = (vec![], vec![]);
    for group in children(ours, groups, groups_end, depth(&ours[groups])) {
        let end = subtree_end(ours, group);
        let heading = positions(ours, group, end, |l| has_class(l, "h3", "exchange-group__heading")).first().and_then(|h| words(&ours[h + 1])).unwrap_or_default();
        let grid = positions(ours, group, end, |l| has_class(l, "div", "exchange-grid")).first().copied().ok_or("a venue group without its grid")?;
        titles.push(heading);
        drawn.push(options(ours, grid));
    }
    let title = |key: &str| deltabadger::web::i18n::text(locale, &format!("bot.setup.venue_groups.{key}"), &[]);
    // No venue at all (nothing the basket trades on): no group, where Rails' grid is empty.
    let want: Vec<String> = match drawn.len() { 2 => vec![title("brokers"), title("exchanges")], 0 | 1 => vec![], n => return Err(format!("{n} venue groups")) };
    if drawn.len() == 2 && titles != want { return Err(format!("the venue groups' headings: {titles:?}, not {want:?}")) }
    if drawn.len() == 1 && ![title("brokers"), title("exchanges")].contains(&titles[0]) { return Err(format!("the venue group's heading: {:?}", titles[0])) }
    // Each group in Rails' order, and the two together exactly Rails' tiles.
    let in_order = |group: &[Vec<Value>]| group.iter().map(|o| rails.iter().position(|r| r == o)).collect::<Option<Vec<_>>>().is_some_and(|at| at.windows(2).all(|w| w[0] < w[1]));
    if !drawn.iter().all(|group| in_order(group)) || drawn.iter().map(Vec::len).sum::<usize>() != rails.len() {
        return Err("the venue groups are not Rails' tiles, each group in Rails' order".into());
    }
    let back = reindent(&theirs[grid..subtree_end(theirs, grid)], depth(&ours[groups]));
    ours.splice(groups..groups_end, back);
    Ok(())
}

/// D-WIZARD-SHEET on one answer to `path` (see the module). Ok(1) when it was a sheet put back as Rails' step, Ok(0) when the
/// answer is not a DCA step's (not a DCA path, or no step in either body), Err when this crate's answer draws a step that is
/// not the sheet it must be.
pub fn wizard_sheet(path: &str, flow: Option<&str>, rails: &mut Value, rust: &mut Value) -> Result<usize, String> {
    if !dca_step(path) { return Ok(0) }
    let (Value::Array(theirs), Value::Array(ours)) = (rails, rust) else { return Ok(0) };
    let layout = |lines: &[Value]| positions(lines, 0, lines.len(), |l| has_class(l, "div", "bot-creation-layout"));
    let [step] = layout(theirs)[..] else { return Ok(0) };
    let modal = |l: &Value| opens(l, "turbo-frame") && attr(text(l), "id").as_deref() == Some("modal");
    let stream = |l: &Value| opens(l, "turbo-stream") && attr(text(l), "target").as_deref() == Some("modal_content");
    // Rails' shell: the modal frame, or the stream's template.
    let (shell, streamed) = match (enclosing(theirs, step, modal), enclosing(theirs, step, stream)) {
        (Some(frame), None) => (frame, false),
        (None, Some(at)) => (enclosing(theirs, step, |l| opens(l, "template")).filter(|t| *t > at).ok_or("a stream without a template")?, true),
        _ => return Err("Rails' step is neither in the modal frame nor in a stream".into()),
    };
    // No step at all is another answer (a venue this build does not trade is refused); the comparison holds it as it did.
    if layout(ours).is_empty() { return Ok(0) }
    let ours_at = if streamed { positions(ours, 0, ours.len(), stream).first().and_then(|s| positions(ours, *s, subtree_end(ours, *s), |l| opens(l, "template")).first().copied()) }
                  else { positions(ours, 0, ours.len(), modal).first().copied() };
    let ours_shell = ours_at.ok_or("no sheet where Rails has its step")?;
    let ours_end = subtree_end(ours, ours_shell);
    let [ours_step] = layout(&ours[ours_shell..ours_end])[..] else { return Err("this crate's sheet holds no step, or more than one".into()) };
    let ours_step = ours_shell + ours_step;
    let tracks_at = positions(ours, ours_shell, ours_end, |l| has_class(l, "ol", "sheet__track"));
    let [track_at] = tracks_at[..] else { return Err(format!("this crate's sheet holds {} tracks", tracks_at.len())) };
    // The track, against Rails' progress bar and sentence.
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
        let rails_shell = without_space(&template(vec![json!("<x-layout>"), json!("</x-layout>")]));
        if theirs_frame != rails_shell { return Err(format!("Rails' stream holds more than its step: {theirs_frame:?}")) }
    } else {
        let close_url = positions(theirs, shell, subtree_end(theirs, shell), |l| opens(l, "dialog")).first().and_then(|d| attr(text(&theirs[*d]), "data-modal--base-close-url-value")).unwrap_or_default();
        let close = deltabadger::web::i18n::text(locale, "button.close", &[]);
        if ours_frame != without_space(&sheet_shell(&close_url, &close)) { return Err(format!("this crate's dialog is not the sheet: {ours_frame:?}")) }
        if theirs_frame != without_space(&full_page_shell(&close_url)) { return Err(format!("Rails' dialog is not the full-page shell: {theirs_frame:?}")) }
    }
    // The pane's search box and Next button: each exactly the form Rails' sentence had for it, then taken out.
    let ours_step_end = subtree_end(ours, ours_step);
    let searches = positions(ours, ours_step, ours_step_end, |l| has_class(l, "div", "sheet__search"));
    let rails_search = sentence.as_ref().and_then(|(_, s)| s.search.as_ref());
    match (searches.as_slice(), rails_search) {
        ([], None) => {}
        ([at], Some(want)) => {
            let form = positions(ours, *at, subtree_end(ours, *at), |l| opens(l, "form"))[0];
            let got = request(ours, form);
            if got.method != want.method || got.url != want.url || got.frame != want.frame || got.search != want.search || !got.fields.is_empty() {
                return Err(format!("the pane's search is not the sentence's: {got:?} / {want:?}"));
            }
        }
        (found, want) => return Err(format!("{} search boxes in the pane where Rails' sentence had {}", found.len(), if want.is_some() { "one" } else { "none" })),
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
    for at in nexts.iter().chain(&searches).copied().rev().collect::<std::collections::BTreeSet<_>>().into_iter().rev() { cut(ours, at) }
    let ours_end = subtree_end(ours, ours_shell);
    let ours_step = positions(ours, ours_shell, ours_end, |l| has_class(l, "div", "bot-creation-layout"))[0];
    // D-WIZARD-VENUE-GROUPS: the brokers and the exchanges under their headings; read back as Rails' one grid.
    ungroup(theirs, step, ours, ours_step, locale)?;
    let ours_end = subtree_end(ours, ours_shell);
    // The basket is drawn open; Rails' was closed unless asked for.
    let list = |l: &Value| opens(l, "div") && attr(text(l), "class").is_some_and(|c| c.split(' ').any(|c| c == "wizard-assets__list"));
    if positions(ours, ours_step, subtree_end(ours, ours_step), list).iter().any(|at| attr(text(&ours[*at]), "class").is_some_and(|c| c.split(' ').any(|c| c == "hidden"))) {
        return Err("this crate's basket is hidden".into());
    }
    for at in positions(theirs, step, step_end, list) { theirs[at] = json!(text(&theirs[at]).replacen("class=\"wizard-assets__list hidden\"", "class=\"wizard-assets__list \"", 1)) }
    // Rails' step without its progress bar and its sentence (and the whitespace before each).
    if let Some((at, _)) = sentence { cut(theirs, at) }
    let (bar, _, _) = progress(theirs, step, subtree_end(theirs, step))?;
    cut(theirs, bar);
    let (step_end, shell_end) = (subtree_end(theirs, step), subtree_end(theirs, shell));
    // This crate's shell is Rails', around this crate's step.
    let rebuilt = [theirs[shell..step].to_vec(), reindent(&ours[ours_step..subtree_end(ours, ours_step)], depth(&theirs[step])), theirs[step_end..shell_end].to_vec()].concat();
    ours.splice(ours_shell..ours_end, rebuilt);
    Ok(1)
}

/// D-WIZARD-SHEET on the chooser: the DCA tile opens the sheet in the modal frame (`data-turbo-frame="modal"`) with no
/// `data-wizard-dir`; put back as Rails' tile. Ok(1) when it was there, Err when the tile is there in another shape.
pub fn chooser_tile(path: &str, rust: &mut Value) -> Result<usize, String> {
    if route(path) != "/bots/new" { return Ok(0) }
    let Value::Array(ours) = rust else { return Ok(0) };
    let tile = |l: &Value| has_class(l, "a", "itile") && attr(text(l), "href").is_some_and(|h| h.ends_with("/bots/dca_single_assets/pick_exchange/new"));
    let [at] = positions(ours, 0, ours.len(), tile)[..] else { return Ok(0) };
    let line = text(&ours[at]).to_string();
    if !line.contains(" data-turbo-frame=\"modal\" ") || line.contains("data-wizard-dir") { return Err(format!("the DCA tile does not open the sheet: {line}")) }
    ours[at] = json!(line.replacen(" data-turbo-frame=\"modal\" ", " data-turbo-frame=\"modal_content\" data-wizard-dir=\"enter\" ", 1));
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
    /// Rails' sentence on the exchange-first key step with nothing chosen: the venue filled (back), the assets' order switch.
    fn key_sentence() -> String {
        format!("<div class=\"conversational conversational--bot-setup\"><span class=\"conversational__lead\">Buy</span><div class=\"conversational__assets\">{}</div><span class=\"conversational__lead\">on</span><form class=\"conversational__link filled\" data-turbo-frame=\"modal_content\" data-wizard-dir=\"back\" action=\"{EXCHANGES}\" accept-charset=\"UTF-8\" method=\"get\"><input value=\"Kraken\" readonly tabindex=\"-1\"></form></div>",
                post("ticker ticker--placeholder ticker--switch", ORDER, &[("flow", "asset_first")], false, "placeholder=\"\""))
    }
    /// Rails' sentence on the asset step, exchange first, BTC and ETH chosen on Kraken: the venue goes back by the advance with
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
        format!("<div class=\"wizard-assets__list {}\" data-class-toggle-target=\"togglable\"><div class=\"wizard-assets__row\">BTC</div></div>", if hidden { "hidden" } else { "" })
    }
    fn rails_step(title: &str, share: usize, sentence: &str, inner: &str) -> String {
        format!("<div class=\"bot-creation-layout\">\n    {}\n\n    {sentence}\n\n    {}\n  </div>\n", bar(title, share), body(inner))
    }
    fn ours_step(extras: &str, inner: &str) -> String { format!("<div class=\"bot-creation-layout\">\n    {extras}\n\n    {}\n  </div>\n", body(inner)) }
    fn page(modal: &str) -> Value { json!(html::normalize(&format!("<html><body><main>bots</main>{modal}</body></html>"))) }
    fn rails_page(content: &str) -> Value {
        // As rails_answer has it, the backdrop's click put on (D-FULL-PAGE-DIALOG-BACKDROP-CLOSES).
        page(&include_str!("../../templates/wizard/full_page.html").replace("{{ close_url }}", "/bots").replace("{% include \"svg/_24x24_close.html\" %}", include_str!("../../templates/svg/_24x24_close.html"))
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
    fn next(fields: &str) -> String { format!("<form class=\"button_to sheet__next\" data-turbo-frame=\"modal_content\" method=\"post\" action=\"{ADVANCE}\"><button type=\"submit\">Next</button>{CSRF}{fields}</form>") }
    fn asset_track(exchange_fields: &[(&str, &str)]) -> String {
        track(&[item("done", "Kraken", Some(("post", ADVANCE, exchange_fields, true))), item("current", "BTC</span><span>ETH", None), item("upcoming", "Currency", Some(("post", ADVANCE, &[], false)))])
    }
    const ASSET_PATH: &str = "/bots/dca_single_assets/pick_buyable_asset/new";

    #[test]
    fn the_first_screen_switches_the_order_from_the_track_and_has_no_search() {
        let rails = || rails_page(&rails_step("Pick exchange", 25, &invite_sentence(), ""));
        let plain_track = || track(&[item("current", "Exchange", None), item("upcoming", "Assets", Some(("post", ORDER, &[("flow", "asset_first")], false))), item("upcoming", "Currency", None)]);
        let (mut theirs, mut ours) = (rails(), page(&sheet("modal_content", &plain_track(), &ours_step("", ""))));
        assert_eq!(wizard_sheet(EXCHANGES, None, &mut theirs, &mut ours), Ok(1));
        assert!(same(&theirs, &ours), "{theirs:#}\n{ours:#}");
        assert!(!theirs.to_string().contains("process-progress") && !theirs.to_string().contains("conversational"));
        // Rails had no search on this screen.
        let with_search = ours_step(&search("exchange-picker", EXCHANGES, "exchange"), "");
        assert!(wizard_sheet(EXCHANGES, None, &mut rails(), &mut page(&sheet("modal_content", &plain_track(), &with_search))).is_err());
        // The switch with the other flow, or as a GET, is not the sentence's.
        for wrong in [item("upcoming", "Assets", Some(("post", ORDER, &[("flow", "exchange_first")], false))), item("upcoming", "Assets", Some(("get", ORDER, &[], false))), item("upcoming", "Assets", None)] {
            let drawn = track(&[item("current", "Exchange", None), wrong, item("upcoming", "Currency", None)]);
            assert!(wizard_sheet(EXCHANGES, None, &mut rails(), &mut page(&sheet("modal_content", &drawn, &ours_step("", "")))).is_err());
        }
    }

    #[test]
    fn the_asset_step_shows_the_tickers_opens_the_basket_and_mirrors_each_slot() {
        let rails = |hidden: bool| rails_page(&rails_step("Pick asset", 75, &asset_sentence(), &basket(hidden)));
        let ours = |track: String, extras: String, hidden: bool| page(&sheet("modal_content", &track, &ours_step(&extras, &basket(hidden))));
        for hidden in [true, false] {
            let (mut theirs, mut drawn) = (rails(hidden), ours(asset_track(&[("to", "exchange")]), next(""), false));
            assert_eq!(wizard_sheet(ASSET_PATH, Some("exchange_first"), &mut theirs, &mut drawn), Ok(1), "basket hidden in Rails: {hidden}");
            assert!(same(&theirs, &drawn), "{theirs:#}\n{drawn:#}");
        }
        let refused = |track: String, extras: String, hidden: bool| wizard_sheet(ASSET_PATH, Some("exchange_first"), &mut rails(true), &mut ours(track, extras, hidden));
        assert!(refused(asset_track(&[("to", "exchange")]), next(""), true).is_err(), "a hidden basket");
        assert!(refused(asset_track(&[("to", "assets")]), next(""), false).is_err(), "a changed `to`");
        assert!(refused(asset_track(&[]), next(""), false).is_err(), "the back without `to`");
        assert!(refused(asset_track(&[("to", "exchange")]), next("<input type=\"hidden\" name=\"to\" value=\"exchange\" />"), false).is_err(), "Next with `to`");
        assert!(refused(asset_track(&[("to", "exchange")]), format!("{}{}", next(""), next("")), false).is_err(), "two Next");
        assert!(refused(asset_track(&[("to", "exchange")]), format!("{}{}", next(""), search("asset-picker", "/q", "USD")), false).is_err(), "a search Rails had not");
        let unnamed = track(&[item("done", "Exchange", Some(("post", ADVANCE, &[("to", "exchange")], true))), item("current", "BTC</span><span>ETH", None), item("upcoming", "Currency", Some(("post", ADVANCE, &[], false)))]);
        assert!(refused(unnamed, next(""), false).is_err(), "the venue's name not shown");
        let no_tickers = track(&[item("done", "Kraken", Some(("post", ADVANCE, &[("to", "exchange")], true))), item("current", "Assets", None), item("upcoming", "Currency", Some(("post", ADVANCE, &[], false)))]);
        assert!(refused(no_tickers, next(""), false).is_err(), "the tickers not shown");
        let two = format!("{}{}", asset_track(&[("to", "exchange")]), asset_track(&[("to", "exchange")]));
        assert!(refused(two, next(""), false).is_err(), "two tracks");
        // A changed field inside the step passes the adapter and is then a difference.
        let (mut theirs, mut drawn) = (rails(false), page(&sheet("modal_content", &asset_track(&[("to", "exchange")]), &ours_step(&next(""), &basket(false)).replace("value=\"1\"", "value=\"2\""))));
        assert_eq!(wizard_sheet(ASSET_PATH, Some("exchange_first"), &mut theirs, &mut drawn), Ok(1));
        assert!(!same(&theirs, &drawn));
        assert!(wizard_sheet(ASSET_PATH, None, &mut rails(false), &mut page(&sheet("modal_frame", &asset_track(&[("to", "exchange")]), &ours_step(&next(""), &basket(false))))).is_err(), "a wrong frame id");
    }

    #[test]
    fn the_key_steps_stream_carries_the_track_and_its_current_item_goes_back() {
        let stream = |inner: &str| json!(html::normalize(&format!("<turbo-stream action=\"update\" target=\"modal_content\"><template>{inner}\n</template></turbo-stream>\n<turbo-stream action=\"prepend\" target=\"flash\"><template><div>no</div></template></turbo-stream>")));
        let key_track = |exchange: Act| track(&[item("current", "Kraken", exchange), item("upcoming", "Assets", Some(("post", ORDER, &[("flow", "asset_first")], false))), item("upcoming", "Currency", None)]);
        let frame = |track: String| include_str!("../../templates/wizard/_sheet_frame.html").replace("{{ track|safe }}", &track).replace("{{ content|safe }}", &ours_step("", ""));
        let rails = || stream(&rails_step("Connect exchange", 50, &key_sentence(), ""));
        let (mut theirs, mut ours) = (rails(), stream(&frame(key_track(Some(("get", EXCHANGES, &[], true))))));
        assert_eq!(wizard_sheet("/bots/dca_single_assets/add_api_key", None, &mut theirs, &mut ours), Ok(1));
        assert!(same(&theirs, &ours), "{theirs:#}\n{ours:#}");
        assert!(wizard_sheet("/bots/dca_single_assets/add_api_key", None, &mut rails(), &mut stream(&frame(key_track(None)))).is_err(), "the current venue must still go back");
        assert!(wizard_sheet("/bots/dca_single_assets/add_api_key", None, &mut rails(), &mut stream(&frame(key_track(Some(("get", EXCHANGES, &[], false)))))).is_err(), "the direction");
        assert!(wizard_sheet("/bots/dca_single_assets/add_api_key", None, &mut rails(), &mut stream(&ours_step("", ""))).is_err(), "no track");
    }

    #[test]
    fn a_search_answer_is_compared_inside_the_step() {
        let results = |r: &str| format!("<turbo-frame id=\"asset-picker\"><button>{r}</button></turbo-frame>");
        let rails = || rails_page(&rails_step("Pick asset", 75, &asset_sentence(), &results("NVDA")));
        let ours = |r: &str| page(&sheet("modal_content", &asset_track(&[("to", "exchange")]), &ours_step(&next(""), &results(r))));
        let (mut theirs, mut drawn) = (rails(), ours("NVDA"));
        assert_eq!(wizard_sheet("/bots/dca_single_assets/pick_buyable_asset/new?query=nv", Some("exchange_first"), &mut theirs, &mut drawn), Ok(1));
        assert!(same(&theirs, &drawn));
        let (mut theirs, mut drawn) = (rails(), ours("NVDL"));
        assert_eq!(wizard_sheet("/bots/dca_single_assets/pick_buyable_asset/new?query=nv", Some("exchange_first"), &mut theirs, &mut drawn), Ok(1));
        assert!(!same(&theirs, &drawn), "a different result list");
    }

    #[test]
    fn the_exchange_step_groups_brokers_and_exchanges_in_rails_order() {
        let tile = |id: &str| format!("<div class=\"exchange-grid__option\"><button name=\"p\" type=\"submit\" value=\"{id}\" class=\"exchange-grid__item\" data-turbo-frame=\"modal_content\">{id}</button></div>");
        let rails_grid = format!("<div class=\"exchange-grid\">{}{}{}</div>", tile("1"), tile("14"), tile("3"));
        let group = |title: &str, ids: &[&str]| format!("<section class=\"exchange-group\"><h3 class=\"exchange-group__heading\">{title}</h3><div class=\"exchange-grid\">{}</div></section>", ids.iter().map(|id| tile(id)).collect::<String>());
        let groups = |a: String, b: String| format!("<div class=\"exchange-groups\">{a}{b}</div>");
        let rails = || rails_page(&rails_step("Pick exchange", 25, &invite_sentence(), &rails_grid));
        let plain_track = track(&[item("current", "Exchange", None), item("upcoming", "Assets", Some(("post", ORDER, &[("flow", "asset_first")], false))), item("upcoming", "Currency", None)]);
        let ours = |inner: String| page(&sheet("modal_content", &plain_track, &ours_step("", &inner)));
        let (mut theirs, mut drawn) = (rails(), ours(groups(group("Brokers", &["14"]), group("Exchanges", &["1", "3"]))));
        assert_eq!(wizard_sheet(EXCHANGES, None, &mut theirs, &mut drawn), Ok(1));
        assert!(same(&theirs, &drawn), "{theirs:#}\n{drawn:#}");
        // No venue: no group, Rails' grid empty.
        let (mut theirs, mut drawn) = (rails_page(&rails_step("Pick exchange", 25, &invite_sentence(), "<div class=\"exchange-grid\"></div>")), ours(groups(String::new(), String::new())));
        assert_eq!(wizard_sheet(EXCHANGES, None, &mut theirs, &mut drawn), Ok(1));
        assert!(same(&theirs, &drawn));
        assert!(wizard_sheet(EXCHANGES, None, &mut rails(), &mut ours(groups(String::new(), String::new()))).is_err(), "Rails' tiles missing");
        for wrong in [groups(group("Brokers", &["14"]), group("Exchanges", &["3", "1"])), groups(group("Brokers", &["14"]), group("Exchanges", &["1"])),
                      groups(group("Exchanges", &["14"]), group("Brokers", &["1", "3"])), groups(group("Brokers", &["14"]), group("Exchanges", &["1", "3", "9"]))] {
            assert!(wizard_sheet(EXCHANGES, None, &mut rails(), &mut ours(wrong.clone())).is_err(), "{wrong}");
        }
    }

    #[test]
    fn the_broker_step_has_a_plain_track() {
        let rails = || rails_page(&format!("<div class=\"bot-creation-layout\">\n    {}\n\n    {}\n  </div>\n", bar("Pick exchange", 50), body("")));
        let plain = track(&[item("done", "BTC", None), item("current", "Exchange", None), item("upcoming", "Currency", None)]);
        let (mut theirs, mut ours) = (rails(), page(&sheet("modal_content", &plain, &ours_step("", ""))));
        assert_eq!(wizard_sheet("/bots/dca_multi_assets/pick_stock_broker/new", Some("asset_first"), &mut theirs, &mut ours), Ok(1));
        assert!(same(&theirs, &ours));
        let linked = track(&[item("done", "BTC", Some(("get", ASSETS, &[], true))), item("current", "Exchange", None), item("upcoming", "Currency", None)]);
        assert!(wizard_sheet("/bots/dca_multi_assets/pick_stock_broker/new", Some("asset_first"), &mut rails(), &mut page(&sheet("modal_content", &linked, &ours_step("", "")))).is_err(),
                "Rails offered no way back there");
    }

    #[test]
    fn index_signal_lazy_pages_and_refusals_are_not_touched() {
        for path in ["/bots/dca_indexes/pick_exchange/new", "/bots/signals/pick_buyable_asset/new", "/de/bots/dca_indexes/add_api_key"] {
            let (mut theirs, mut ours) = (rails_page(&rails_step("Pick exchange", 25, &invite_sentence(), "")), page("<div>other</div>"));
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
        let (mut theirs, mut ours) = (rails_page(&rails_step("Connect exchange", 50, &key_sentence(), "")), refusal.clone());
        assert_eq!(wizard_sheet("/bots/dca_single_assets/add_api_key/new", None, &mut theirs, &mut ours), Ok(0));
        assert_eq!(ours, refusal);
    }

    #[test]
    fn the_choosers_dca_tile_is_put_back_as_rails_writes_it() {
        let tile = |attrs: &str| json!(html::normalize(&format!("<a class=\"itile itile--bot-type\" {attrs} href=\"/de/bots/dca_single_assets/pick_exchange/new\">DCA</a><a class=\"itile\" data-turbo-frame=\"modal_content\" data-wizard-dir=\"enter\" href=\"/bots/dca_indexes/setup_coingecko/new\">Index</a>")));
        let mut ours = tile("data-turbo-frame=\"modal\"");
        assert_eq!(chooser_tile("/de/bots/new", &mut ours), Ok(1));
        assert_eq!(ours, tile("data-turbo-frame=\"modal_content\" data-wizard-dir=\"enter\""));
        assert!(chooser_tile("/bots/new", &mut tile("data-turbo-frame=\"modal_content\" data-wizard-dir=\"enter\"")).is_err(), "the old tile");
    }
}
