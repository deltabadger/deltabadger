//! The page-parity normaliser: turns a response into lines that are equal exactly when Rails and Rust
//! sent the same page. Both sides go through this same code.
//!
//! The body is parsed as a browser parses it (html5ever), then written one node per line:
//! - an element as `<name attr="value" …>` with its attributes sorted by name, values exact;
//! - text with every run of HTML whitespace (space, tab, line feed, form feed, carriage return)
//!   collapsed to one space; a text node that is only whitespace is kept as one space, so "some
//!   whitespace between two tags" and "none" stay different. A non-breaking space, or any other
//!   character a browser renders, is text and is kept. Inside `pre`, `textarea`, `script` and
//!   `style`, where whitespace is content, nothing is collapsed;
//! - comments and the doctype as they are.
//!
//! Values that differ on every request, or by design, are masked:
//! - the CSRF token (`meta[name=csrf-token]`, `input[name=authenticity_token]`) -> `[csrf]`;
//! - the CSP nonce (`meta[name=csp-nonce]`, any `nonce` attribute) -> `[nonce]`;
//! - an asset fingerprint (`/assets/…-<8 to 64 hex>.<ext>` in any attribute) -> `-[digest]`;
//! - a signed stream name -> `[signed <the stream name inside>]`, so the name is compared and the
//!   signature, which each side makes with its own key, is not.
//!
//! Masking hides whether a token or a signature is genuine, and whether an asset exists.
//! `masked_values` hands them out so the harness can check that first, on each side, with that
//! side's own verifier.
//!
//! `location` does the same for a `Location` header: Rails sends an absolute URL and this crate a
//! path, and they are equal only when they name the same page of this deployment.
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use scraper::{Html, Node};

/// HTML's own whitespace only: U+00A0 and the other Unicode spaces are characters the reader sees.
fn collapse(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r') {
            if !out.ends_with(' ') { out.push(' '); }
        } else {
            out.push(c);
        }
    }
    out
}

/// Elements whose text is kept exactly.
const VERBATIM: [&str; 4] = ["pre", "textarea", "script", "style"];

fn mask_digest(value: &str) -> String {
    let Some(at) = value.find("/assets/") else { return value.to_string() };
    let (path, query) = value[at..].split_once('?').map_or((&value[at..], ""), |(p, q)| (p, q));
    let Some((stem, extension)) = path.rsplit_once('.') else { return value.to_string() };
    match stem.rsplit_once('-') {
        Some((name, digest)) if (8..=64).contains(&digest.len()) && digest.bytes().all(|b| b.is_ascii_hexdigit()) => {
            format!("{}{name}-[digest].{extension}{}{query}", &value[..at], if query.is_empty() { "" } else { "?" })
        }
        _ => value.to_string(),
    }
}

fn signed_name(value: &str) -> String {
    let name = value.split_once("--")
        .and_then(|(data, _)| B64.decode(data).ok())
        .and_then(|json| serde_json::from_slice::<serde_json::Value>(&json).ok())
        .and_then(|v| v.as_str().map(str::to_string));
    format!("[signed {}]", name.unwrap_or_else(|| format!("UNREADABLE {value}")))
}

fn attribute(element: &scraper::node::Element, name: &str, value: &str) -> String {
    let is = |attr: &str, expected: &str| element.attr(attr) == Some(expected);
    match (element.name(), name) {
        ("meta", "content") if is("name", "csrf-token") => "[csrf]".into(),
        ("input", "value") if is("name", "authenticity_token") => "[csrf]".into(),
        ("meta", "content") if is("name", "csp-nonce") => "[nonce]".into(),
        (_, "nonce") => "[nonce]".into(),
        ("turbo-cable-stream-source", "signed-stream-name") => signed_name(value),
        _ => mask_digest(value),
    }
}

fn walk(node: ego_tree::NodeRef<'_, Node>, depth: usize, lines: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    match node.value() {
        Node::Doctype(doctype) => lines.push(format!("{indent}<!DOCTYPE {}>", doctype.name())),
        Node::Comment(comment) => lines.push(format!("{indent}<!--{}-->", &**comment)),
        Node::Text(text) => {
            let verbatim = node.ancestors().any(|a| a.value().as_element().is_some_and(|e| VERBATIM.contains(&e.name())));
            let shown = if verbatim { text.to_string() } else { collapse(text) };
            if !shown.is_empty() { lines.push(format!("{indent}{shown:?}")); }
        }
        Node::Element(element) => {
            let mut attributes: Vec<(&str, String)> = element.attrs().map(|(name, value)| (name, attribute(element, name, value))).collect();
            attributes.sort();
            let attributes: String = attributes.iter().map(|(name, value)| format!(" {name}={value:?}")).collect();
            lines.push(format!("{indent}<{}{attributes}>", element.name()));
            for child in node.children() { walk(child, depth + 1, lines); }
            lines.push(format!("{indent}</{}>", element.name()));
        }
        _ => for child in node.children() { walk(child, depth, lines); },
    }
}

pub fn normalize(body: &str) -> Vec<String> {
    let mut lines = Vec::new();
    walk(Html::parse_document(body).tree.root(), 0, &mut lines);
    lines
}

/// The `content` of the first `<meta name="NAME">`: how the tests read the nonce a page carries.
pub fn meta(body: &str, name: &str) -> Option<String> {
    let document = Html::parse_document(body);
    let selector = scraper::Selector::parse(&format!("meta[name=\"{name}\"]")).ok()?;
    document.select(&selector).next()?.value().attr("content").map(str::to_string)
}

/// Every value on the page that the masks above hide, unmasked, so a test can check each one is
/// genuine before it is masked.
pub struct Masked {
    pub tokens: Vec<String>,
    /// Signed stream names.
    pub streams: Vec<String>,
    /// Every `/assets/...` path any attribute refers to, without its query.
    pub assets: Vec<String>,
}

pub fn masked_values(body: &str) -> Masked {
    let document = Html::parse_document(body);
    let values = |css: &str, attribute: &str| -> Vec<String> {
        let selector = scraper::Selector::parse(css).expect("a valid selector");
        document.select(&selector).filter_map(|e| e.value().attr(attribute).map(str::to_string)).collect()
    };
    let mut tokens = values("meta[name=\"csrf-token\"]", "content");
    tokens.extend(values("input[name=\"authenticity_token\"]", "value"));
    let everything = scraper::Selector::parse("*").expect("a valid selector");
    let assets = document.select(&everything)
        .flat_map(|element| element.value().attrs().map(|(_, value)| value))
        .filter_map(|value| value.find("/assets/").map(|at| value[at..].split('?').next().unwrap_or_default().to_string()))
        .collect();
    Masked { tokens, streams: values("turbo-cable-stream-source", "signed-stream-name"), assets }
}

/// A `Location` as the comparison sees it. Rails sends an absolute URL on its own origin and this
/// crate a path: the same page, written as its path. Anything else is marked and kept whole, so it
/// equals nothing local: another origin, and above all a value beginning with `//`, which a browser
/// reads as another host (`//bots` is the host `bots`, not the page `/bots`).
/// Only after that check are repeated slashes inside the path squeezed, as Rails' router reads them.
pub fn location(value: &str) -> String {
    let path = match value.strip_prefix("http://localhost:3000") {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '?']) => rest, // an absolute URL on this origin
        None if value.starts_with('/') && !value.starts_with("//") => value,     // a path on this origin
        _ => return format!("[elsewhere] {value}"),
    };
    let (path, query) = path.split_once('?').map_or((path, None), |(path, query)| (path, Some(query)));
    let squeezed = deltabadger::web::normalize_path(&format!("/{path}"));
    query.map_or(squeezed.clone(), |query| format!("{squeezed}?{query}"))
}

/// Where two normalised pages first differ, with a few lines of context from each.
pub fn first_difference(rails: &[String], rust: &[String]) -> Option<String> {
    let at = rails.iter().zip(rust.iter()).position(|(a, b)| a != b).or((rails.len() != rust.len()).then_some(rails.len().min(rust.len())))?;
    let context = |lines: &[String]| lines[at.saturating_sub(3)..(at + 3).min(lines.len())].join("\n    ");
    Some(format!("line {at}\n  rails:\n    {}\n  rust:\n    {}", context(rails), context(rust)))
}
