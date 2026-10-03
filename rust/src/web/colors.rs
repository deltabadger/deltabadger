//! ColorsHelper#ensure_contrast and the ticker pill's class and data attributes (ApplicationHelper).
//! Pinned by the "bot_pages" vectors.

/// TrackerHelper::NEUTRAL_COLOR, and the fallback every pill and logo uses for an asset without a colour.
pub const NEUTRAL: &str = "#8A9BA8";

/// `hex[a..b].hex`: the leading hexadecimal digits of a two-character slice, 0 when there are none.
fn channel(slice: &str) -> f64 {
    let digits: String = slice.chars().take_while(char::is_ascii_hexdigit).collect();
    i64::from_str_radix(&digits, 16).unwrap_or(0) as f64
}

/// A colour light or dark enough to be read on either theme: darkened by a fifth above a luminance
/// of 0.7, lightened by a tenth below 0.3 and by two fifths at 0.04 and below. `None` where Ruby
/// raises: a value with fewer than five characters after its `#` has no blue channel.
pub fn ensure_contrast(color: &str) -> Option<String> {
    if color.trim().is_empty() { return Some(color.to_string()); }
    let hex = color.replace('#', "");
    let slice = |from: usize| hex.char_indices().nth(from).map(|(at, _)| hex.get(at..).unwrap_or("").chars().take(2).collect::<String>());
    let (r, g, b) = (channel(&slice(0)?), channel(&slice(2)?), channel(&slice(4)?));
    let linear = |c: f64| { let c = c / 255.0; if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) } };
    let luminance = 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
    let lighten = |c: f64, by: f64| (c + (255.0 - c) * by).round();
    let (r, g, b) = if luminance > 0.7 {
        ((r * (1.0 - 0.2)).round(), (g * (1.0 - 0.2)).round(), (b * (1.0 - 0.2)).round())
    } else if luminance < 0.3 && luminance > 0.04 {
        (lighten(r, 0.1), lighten(g, 0.1), lighten(b, 0.1))
    } else if luminance <= 0.04 {
        (lighten(r, 0.4), lighten(g, 0.4), lighten(b, 0.4))
    } else {
        (r, g, b)
    };
    Some(format!("#{:02x}{:02x}{:02x}", r as i64, g as i64, b as i64))
}

/// `ensure_contrast(asset.color || '#8A9BA8')`.
pub fn pill_color(color: Option<&str>) -> Option<String> {
    ensure_contrast(color.unwrap_or(NEUTRAL))
}

/// ApplicationHelper#ticker_class_for: only a stock without a colour of its own keeps the stock styling.
pub fn ticker_class(category: Option<&str>, color: Option<&str>) -> &'static str {
    let colored = color.is_some_and(|c| !c.trim().is_empty());
    if !colored && category == Some("Stock") { "ticker ticker--stock" } else { "ticker" }
}
