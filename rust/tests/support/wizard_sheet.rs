//! D-WIZARD-SHEET (named divergence from the 2.x recordings): a DCA wizard step is the sheet Settings uses, its track in the
//! side column and the step in the pane, where Rails draws it in the full-page dialog with a progress bar on top. The step
//! itself (the `.bot-creation-layout` subtree: sentence, table, search, form) is compared as it was, the progress bar left
//! out of Rails' side; around it, this crate's lines must be exactly the sheet (`wizard/sheet.html`, or for the key step's
//! 422 stream `wizard/_sheet_frame.html`) with one track that is the step's, and Rails' exactly the full-page shell. Then
//! this crate's shell is put back as Rails' around this crate's step, so the page around the modal frame, the step and
//! everything else are compared as before. On the chooser only the DCA tile's frame target and its dropped
//! `data-wizard-dir` are left out. The index and signal wizards, and a lazy page of results (no shell), are not touched.
use super::{attr, html, is_space, subtree_end};
use serde_json::{json, Value};

const LOCALES: [&str; 15] = ["en", "de", "fr", "it", "es", "pt", "nl", "pl", "ru", "bg", "el", "sv", "da", "cs", "sk"];
/// The track's labels by the step's place (D-WIZARD-TRACK), in each order.
const EXCHANGE_FIRST: [&str; 4] = ["pick_exchange", "api", "assets", "spendable"];
const ASSET_FIRST: [&str; 4] = ["assets", "pick_exchange", "api", "spendable"];

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

/// The track `labels` (the steps in their order) at `current`, as `wizard/_track.html` draws it, without the whitespace.
fn track(locale: &str, labels: &[&str], current: usize) -> Vec<Value> {
    let items: String = labels.iter().enumerate().map(|(index, key)| {
        let (class, aria) = match index.cmp(&current) {
            std::cmp::Ordering::Less => (" sheet__step--done", ""),
            std::cmp::Ordering::Equal => (" sheet__step--current", " aria-current=\"step\""),
            std::cmp::Ordering::Greater => ("", ""),
        };
        let label = deltabadger::web::i18n::text(locale, &format!("bot.setup.progress_steps.{key}"), &[]);
        format!("<li class=\"sheet__step{class}\"{aria}><span>{}</span></li>", escape(&label))
    }).collect();
    body_of(&html::normalize(&format!("<ol class=\"sheet__track\">{items}</ol>")))
}

fn without_space(lines: &[Value]) -> Vec<Value> { lines.iter().filter(|l| !is_space(l)).cloned().collect() }

/// Rails' progress bar in its step: the title and the share, 25 to 100.
fn progress(layout: &[Value]) -> Result<(String, usize), String> {
    let bar = layout.iter().position(|l| has_class(l, "div", "process-progress")).ok_or("Rails' step has no progress bar")?;
    let end = subtree_end(layout, bar);
    let title = layout[bar..end].iter().position(|l| opens(l, "h4")).map(|at| text(&layout[bar + at + 1]).trim().trim_matches('"').to_string()).ok_or("no title")?;
    let share = layout[bar..end].iter().find(|l| has_class(l, "div", "slider__style__progress")).and_then(|l| attr(text(l), "style"))
        .and_then(|style| style.strip_prefix("width: ")?.strip_suffix('%')?.parse().ok()).ok_or("no share")?;
    Ok((title, share))
}

/// The tracks this step may show, by Rails' progress bar (its title names the step, its share the place), each with the locale
/// it is in. `flow`: the wizard's order as the session holds it, when it holds one.
fn tracks(layout: &[Value], flow: Option<&str>) -> Result<Vec<(&'static str, Vec<Value>)>, String> {
    let (title, share) = progress(layout)?;
    let place = (share / 25).checked_sub(1).filter(|place| *place < 4 && share % 25 == 0).ok_or(format!("a share of {share}%"))?;
    let mut out = vec![];
    for locale in LOCALES {
        for (key, label) in [("pick_exchange", "pick_exchange"), ("currencies", "assets"), ("assets", "assets"), ("api", "api"), ("spendable", "spendable")] {
            if deltabadger::web::i18n::text(locale, &format!("bot.setup.progress_steps.{key}"), &[]) != title { continue }
            for (order, labels) in [("exchange_first", EXCHANGE_FIRST), ("asset_first", ASSET_FIRST)] {
                if labels[place] != label || flow.is_some_and(|flow| flow != order) { continue }
                out.push((locale, track(locale, &labels, place)));
            }
        }
    }
    if out.is_empty() { return Err(format!("no DCA step reads \"{title}\" at {share}%")) }
    Ok(out)
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
    let shell_end = subtree_end(theirs, shell);
    let tracks = tracks(&theirs[step..subtree_end(theirs, step)], flow)?;
    // This crate's: the same, with one step and one track in it.
    let ours_at = if streamed { positions(ours, 0, ours.len(), stream).first().and_then(|s| positions(ours, *s, subtree_end(ours, *s), |l| opens(l, "template")).first().copied()) }
                  else { positions(ours, 0, ours.len(), modal).first().copied() };
    // No step at all is another answer (a venue this build does not trade is refused); the comparison holds it as it did.
    if layout(ours).is_empty() { return Ok(0) }
    let ours_shell = ours_at.ok_or("no sheet where Rails has its step")?;
    let ours_end = subtree_end(ours, ours_shell);
    let [ours_step] = layout(&ours[ours_shell..ours_end])[..] else { return Err("this crate's sheet holds no step, or more than one".into()) };
    let ours_step = ours_shell + ours_step;
    let tracks_at = positions(ours, ours_shell, ours_end, |l| has_class(l, "ol", "sheet__track"));
    let [track_at] = tracks_at[..] else { return Err(format!("this crate's sheet holds {} tracks", tracks_at.len())) };
    let drawn = without_space(&reindent(&ours[track_at..subtree_end(ours, track_at)], 0));
    // A locale that has no label of its own reads English's: each locale whose track it is may be the page's.
    let locales: Vec<&str> = tracks.iter().filter(|(_, track)| without_space(track) == drawn).map(|(locale, _)| *locale).collect();
    if locales.is_empty() {
        return Err(format!("the track is not the step's:\n  drawn: {drawn:?}\n  any of: {:?}", tracks.iter().map(|(_, t)| without_space(t)).collect::<Vec<_>>()));
    }
    let mut holes = [(track_at, "track"), (ours_step, "layout")];
    holes.sort();
    let ours_frame = without_space(&with_holes(ours, ours_shell, ours_end, &holes));
    let theirs_frame = without_space(&with_holes(theirs, shell, shell_end, &[(step, "layout")]));
    if streamed {
        let template = |inner: Vec<Value>| [vec![json!("<template>")], reindent(&inner, 1), vec![json!("</template>")]].concat();
        let sheet = without_space(&template(body_of(&html::normalize(&sheet_frame()))));
        if ours_frame != sheet { return Err(format!("the stream's template is not the sheet's frame: {ours_frame:?}")) }
        let rails_shell = without_space(&template(vec![json!("<x-layout>"), json!("</x-layout>")]));
        if theirs_frame != rails_shell { return Err(format!("Rails' stream holds more than its step: {theirs_frame:?}")) }
    } else {
        let close_url = positions(theirs, shell, shell_end, |l| opens(l, "dialog")).first().and_then(|d| attr(text(&theirs[*d]), "data-modal--base-close-url-value")).unwrap_or_default();
        let sheet = |locale: &&str| without_space(&sheet_shell(&close_url, &deltabadger::web::i18n::text(locale, "button.close", &[])));
        if !locales.iter().any(|locale| ours_frame == sheet(locale)) { return Err(format!("this crate's dialog is not the sheet: {ours_frame:?}")) }
        if theirs_frame != without_space(&full_page_shell(&close_url)) { return Err(format!("Rails' dialog is not the full-page shell: {theirs_frame:?}")) }
    }
    // Rails' step without its progress bar (and the whitespace before it); then this crate's shell is Rails', around this crate's step.
    let step_end = subtree_end(theirs, step);
    let bar = positions(theirs, step, step_end, |l| has_class(l, "div", "process-progress"))[0];
    let bar_start = if is_space(&theirs[bar - 1]) { bar - 1 } else { bar };
    theirs.drain(bar_start..subtree_end(theirs, bar));
    let (step_end, shell_end) = (subtree_end(theirs, step), subtree_end(theirs, shell));
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

    const SENTENCE: &str = "<div class=\"conversational\"><span>Buy</span></div>";
    fn bar(title: &str, share: usize) -> String {
        format!("\n<div class=\"process-progress\">\n  <h4>{title}</h4>\n  <div class=\"slider\"><div class=\"slider__style\"><div class=\"slider__style__track\"><div class=\"slider__style__progress\" style=\"width: {share}%\"></div></div></div></div>\n</div>")
    }
    fn step(field: &str) -> String {
        format!("<div class=\"bot-creation-layout\">\n    {SENTENCE}\n    <div class=\"vt-step-body\"><form action=\"/bots/dca_single_assets/pick_exchange\"><input name=\"{field}\" value=\"1\"></form></div>\n  </div>\n")
    }
    fn rails_step(field: &str, title: &str, share: usize) -> String { step(field).replacen(">\n    ", &format!(">\n    {}\n\n    ", bar(title, share)), 1) }
    fn page(modal: &str) -> Value { json!(html::normalize(&format!("<html><body><main>bots</main>{modal}</body></html>"))) }
    fn rails_page(content: &str) -> Value {
        // As rails_answer has it, the backdrop's click put on (D-FULL-PAGE-DIALOG-BACKDROP-CLOSES).
        page(&include_str!("../../templates/wizard/full_page.html").replace("{{ close_url }}", "/bots").replace("{% include \"svg/_24x24_close.html\" %}", include_str!("../../templates/svg/_24x24_close.html"))
            .replace("{{ content|safe }}", content).replacen(" complete=\"\"", "", 1))
    }
    fn track_html(labels: &[(&str, &str)]) -> String {
        let items: String = labels.iter().map(|(label, state)| match *state {
            "done" => format!("\n  <li class=\"sheet__step sheet__step--done\"><span>{label}</span></li>"),
            "current" => format!("\n  <li class=\"sheet__step sheet__step--current\" aria-current=\"step\"><span>{label}</span></li>"),
            _ => format!("\n  <li class=\"sheet__step\"><span>{label}</span></li>"),
        }).collect();
        format!("<ol class=\"sheet__track\">{items}\n</ol>")
    }
    fn exchange_track() -> String { track_html(&[("Pick exchange", "current"), ("Connect exchange", ""), ("Assets", ""), ("Currency", "")]) }
    fn sheet(frame_id: &str, track: &str, content: &str) -> String { sheet_in("en", frame_id, track, content) }
    fn sheet_in(locale: &str, frame_id: &str, track: &str, content: &str) -> String {
        let frame = include_str!("../../templates/wizard/_sheet_frame.html").replace("{{ track|safe }}", track).replace("{{ content|safe }}", content);
        include_str!("../../templates/wizard/sheet.html").replace("{{ close_url }}", "/bots").replace("{{ close_label }}", &deltabadger::web::i18n::text(locale, "button.close", &[]))
            .replace("{% include \"svg/_24x24_close.html\" %}", include_str!("../../templates/svg/_24x24_close.html"))
            .replace("{% include \"wizard/_sheet_frame.html\" %}", &frame).replace("id=\"modal_content\"", &format!("id=\"{frame_id}\""))
    }
    const PATH: &str = "/bots/dca_single_assets/pick_exchange/new";

    #[test]
    fn only_the_shell_and_the_progress_bar_are_left_out() {
        let mut theirs = rails_page(&rails_step("bots_dca_single_asset[exchange_id]", "Pick exchange", 25));
        let mut ours = page(&sheet("modal_content", &exchange_track(), &step("bots_dca_single_asset[exchange_id]")));
        assert_eq!(wizard_sheet(PATH, None, &mut theirs, &mut ours), Ok(1));
        assert_eq!(without_space(theirs.as_array().unwrap()), without_space(ours.as_array().unwrap()));
        assert!(!theirs.to_string().contains("process-progress"), "the bar is gone from Rails' step");
        // In German, asset-first: the track is the second order's, its labels the locale's.
        let de = |key: &str| deltabadger::web::i18n::text("de", &format!("bot.setup.progress_steps.{key}"), &[]);
        let mut theirs = rails_page(&rails_step("f", &de("pick_exchange"), 50));
        let track = track_html(&[(&de("assets"), "done"), (&de("pick_exchange"), "current"), (&de("api"), ""), (&de("spendable"), "")]);
        let mut ours = page(&sheet_in("de", "modal_content", &track, &step("f")));
        assert_eq!(wizard_sheet(&format!("/de{PATH}"), Some("asset_first"), &mut theirs, &mut ours), Ok(1));
        assert_eq!(without_space(theirs.as_array().unwrap()), without_space(ours.as_array().unwrap()));
    }

    #[test]
    fn the_key_steps_stream_is_the_sheets_frame() {
        let stream = |inner: &str| json!(html::normalize(&format!("<turbo-stream action=\"update\" target=\"modal_content\"><template>{inner}\n</template></turbo-stream>\n<turbo-stream action=\"prepend\" target=\"flash\"><template><div>no</div></template></turbo-stream>")));
        let track = track_html(&[("Pick exchange", "done"), ("Connect exchange", "current"), ("Assets", ""), ("Currency", "")]);
        let mut theirs = stream(&rails_step("api_key[key]", "Connect exchange", 50));
        let frame = include_str!("../../templates/wizard/_sheet_frame.html").replace("{{ track|safe }}", &track).replace("{{ content|safe }}", &step("api_key[key]"));
        let mut ours = stream(&frame);
        assert_eq!(wizard_sheet("/bots/dca_single_assets/add_api_key", None, &mut theirs, &mut ours), Ok(1));
        assert_eq!(without_space(theirs.as_array().unwrap()), without_space(ours.as_array().unwrap()));
        // A stream that still sends the old step: no track.
        let mut ours = stream(&step("api_key[key]"));
        assert!(wizard_sheet("/bots/dca_single_assets/add_api_key", None, &mut stream(&rails_step("api_key[key]", "Connect exchange", 50)), &mut ours).is_err());
    }

    #[test]
    fn a_sheet_that_is_not_the_steps_is_refused() {
        let rails = || rails_page(&rails_step("f", "Pick exchange", 25));
        let refused = |ours: String| wizard_sheet(PATH, None, &mut rails(), &mut page(&ours));
        assert!(refused(rails_step("f", "Pick exchange", 25)).is_err(), "no track: the old full-page step");
        assert!(refused(sheet("modal_content", &format!("{}{}", exchange_track(), exchange_track()), &step("f"))).is_err(), "two tracks");
        assert!(refused(sheet("modal_frame", &exchange_track(), &step("f"))).is_err(), "a wrong frame id");
        assert!(refused(sheet("modal_content", &track_html(&[("Pick exchange", "done"), ("Connect exchange", "current"), ("Assets", ""), ("Currency", "")]), &step("f"))).is_err(), "another step current");
        assert!(refused(sheet("modal_content", &track_html(&[("Pick exchange", "current"), ("Connect exchange", ""), ("Pick asset", ""), ("Currency", "")]), &step("f"))).is_err(), "the asset step's other label");
        // A changed field inside the step passes the adapter and is then a difference.
        let (mut theirs, mut ours) = (rails(), page(&sheet("modal_content", &exchange_track(), &step("g"))));
        assert_eq!(wizard_sheet(PATH, None, &mut theirs, &mut ours), Ok(1));
        assert_ne!(without_space(theirs.as_array().unwrap()), without_space(ours.as_array().unwrap()));
    }

    #[test]
    fn a_search_answer_is_compared_inside_the_step() {
        let search = |result: &str| format!("<div class=\"bot-creation-layout\">\n    {SENTENCE}\n    <turbo-frame id=\"asset-picker\"><form><button value=\"{result}\">{result}</button></form></turbo-frame>\n  </div>\n");
        let path = "/bots/dca_single_assets/pick_buyable_asset/new?query=nv";
        let rails = || rails_page(&search("NVDA").replacen(">\n    ", &format!(">\n    {}\n\n    ", bar("Pick asset", 25)), 1));
        let track = track_html(&[("Assets", "current"), ("Pick exchange", ""), ("Connect exchange", ""), ("Currency", "")]);
        let (mut theirs, mut ours) = (rails(), page(&sheet("modal_content", &track, &search("NVDA"))));
        assert_eq!(wizard_sheet(path, Some("asset_first"), &mut theirs, &mut ours), Ok(1));
        assert_eq!(without_space(theirs.as_array().unwrap()), without_space(ours.as_array().unwrap()));
        let (mut theirs, mut ours) = (rails(), page(&sheet("modal_content", &track, &search("NVDL"))));
        assert_eq!(wizard_sheet(path, Some("asset_first"), &mut theirs, &mut ours), Ok(1));
        assert_ne!(without_space(theirs.as_array().unwrap()), without_space(ours.as_array().unwrap()), "a different result list");
    }

    #[test]
    fn index_signal_lazy_pages_and_refusals_are_not_touched() {
        for path in ["/bots/dca_indexes/pick_exchange/new", "/bots/signals/pick_buyable_asset/new", "/de/bots/dca_indexes/add_api_key"] {
            let (mut theirs, mut ours) = (rails_page(&rails_step("f", "Pick exchange", 25)), page(&sheet("modal_content", &exchange_track(), &step("f"))));
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
        let (mut theirs, mut ours) = (rails_page(&rails_step("f", "Connect exchange", 50)), refusal.clone());
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
