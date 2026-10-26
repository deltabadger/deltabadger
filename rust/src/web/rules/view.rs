use super::{model::Rule, reads::Listing};
use crate::{
    ruby::BigDec,
    web::{
        colors, format,
        i18n::{self, escape, Arg},
        layout::Ctx,
        WebError,
    },
};
use askama::Template;

const WALLET: &str = include_str!("../../../templates/svg/_16x16_wallet.html");

fn input(r: &Rule, key: &str, value: &str, required: bool) -> String {
    let (min, max, step, width, placeholder) = match key {
        "min_amount" => ("1.0e-08", "", "any", 70, ""),
        "max_interval" => ("1", "365", "1", 50, "30"),
        _ => ("0.01", "100", "any", 50, ""),
    };
    let mut attrs = format!(
        "value=\"{}\" class=\"sinput numeric-input\" step=\"{step}\" min=\"{min}\"",
        escape(value)
    );
    if !max.is_empty() {
        attrs += &format!(" max=\"{max}\"");
    }
    attrs += " size=\"2\"";
    if required {
        attrs += " required=\"required\"";
    }
    if r.status == 1 {
        attrs += " disabled=\"disabled\"";
    }
    if !placeholder.is_empty() {
        attrs += &format!(" placeholder=\"{placeholder}\"");
    }
    if key == "max_interval" && value.is_empty() {
        attrs = attrs.replacen("value=\"\" ", "", 1);
    }
    format!("<input {attrs} data-controller=\"autowidth-input\" data-action=\"input-&gt;autowidth-input#resize input-&gt;form--submit-after-delay#submit\" data-form--move-cursor-to-end-target=\"input\" style=\"width: {width}px;\" type=\"number\" name=\"rules_withdrawal[{key}]\" id=\"rules_withdrawal_{key}\" />")
}

pub fn options(list: &Listing, selected: &str) -> String {
    list.entries
        .iter()
        .filter(|e| e.refused.is_none())
        .map(|e| {
            format!(
                "<option {}value=\"{}\">{}</option>",
                if e.address == selected {
                    "selected=\"selected\" "
                } else {
                    ""
                },
                escape(&e.address),
                escape(e.key.as_deref().unwrap_or(&e.address))
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn address(r: &Rule, list: Option<&Listing>, ctx: &Ctx) -> String {
    let name = r.text("address_name");
    let display = if name.trim().is_empty() {
        &r.address
    } else {
        &name
    };
    if r.status == 2 {
        if let Some(list) = list {
            if list.entries.is_empty() {
                return format!("<span class=\"tag tag--address\">{WALLET}</span>");
            }
            return format!("<div class=\"tag tag--address\" data-controller=\"form--select-display\">
{WALLET}<span data-form--select-display-target=\"label\">{}</span>
<select data-action=\"change-&gt;form--select-display#update change-&gt;form--submit#submit\" name=\"rules_withdrawal[address]\" id=\"rules_withdrawal_address\">{}</select>
</div>", escape(display), options(list, &r.address));
        }
    }
    let (network, tag) = (r.text("network"), r.text("address_tag"));
    let tooltip = !name.trim().is_empty() || !network.trim().is_empty() || !tag.trim().is_empty();
    let mut out = if tooltip {
        "<span style=\"position: relative\" data-controller=\"tooltip\" data-action=\"mouseenter-&gt;tooltip#showTooltip mouseleave-&gt;tooltip#hideTooltip\">".into()
    } else {
        String::from("<span>")
    };
    out += &format!(
        "
<span class=\"tag tag--address\">{WALLET}{}</span>",
        escape(display)
    );
    if tooltip {
        out += &format!(
            "
<div class=\"tooltip tooltip--address\">
<div><b>{}</b></div>
",
            escape(&r.address)
        );
        for (key, text) in [("network", network), ("memo", tag)] {
            if !text.trim().is_empty() {
                out += &format!(
                    "<div><b>{}:</b> {}</div>
",
                    ctx.t(&format!("rules.tile.{key}")),
                    escape(&text)
                );
            }
        }
        out += "</div>";
    }
    out + "
</span>"
}

pub fn info(ctx: &Ctx, r: &Rule) -> String {
    let fee = r.fee_amount();
    let symbol = r.asset.symbol();
    let text = if r.threshold() == "min_amount" {
        let amount =
            BigDec::parse(&r.value("min_amount", "0.1")).unwrap_or_else(|_| BigDec::zero());
        if fee.is_positive() && amount.is_positive() {
            let pct = format::float_round(
                (&fee.div(&amount).unwrap_or_else(BigDec::zero) * &BigDec::from_i64(100)).to_f(),
                2,
            );
            let pct = format!("{}%", format::float_to_s(pct));
            let pct = if (&fee.div(&amount).unwrap_or_else(BigDec::zero) * &BigDec::from_i64(100))
                .to_f()
                > 1.0
            {
                format!("<span class=\"text-danger\">{pct}</span>")
            } else {
                pct
            };
            i18n::t(
                ctx.locale,
                "rules.tile.fixed_amount_fee_html",
                &[
                    ("fee", Arg::Text(&fee.to_s_f())),
                    ("symbol", Arg::Text(symbol)),
                    ("fee_pct", Arg::Html(&pct)),
                ],
            )
        } else {
            i18n::t(
                ctx.locale,
                "rules.tile.fixed_amount_info",
                &[
                    (
                        "amount",
                        Arg::Text(
                            &r.minimum()
                                .map(|v| v.to_s_f())
                                .unwrap_or_else(|| r.value("min_amount", "0.1")),
                        ),
                    ),
                    ("symbol", Arg::Text(symbol)),
                ],
            )
        }
    } else if let Some(min) = r.minimum() {
        i18n::t(
            ctx.locale,
            "rules.tile.min_amount_info_html",
            &[
                ("fee", Arg::Text(&fee.to_s_f())),
                ("symbol", Arg::Text(symbol)),
                ("amount", Arg::Text(&min.to_s_f())),
            ],
        )
    } else if r.fee.is_some() && fee.is_zero() {
        i18n::t(
            ctx.locale,
            "rules.tile.no_fee_info",
            &[("exchange", Arg::Text(&r.exchange.name))],
        )
    } else {
        return String::new();
    };
    format!(
        "<div class=\"small-info\">
{text}
</div>"
    )
}

/// E0 supplies an unresolved intent here. Keeping this renderer independent of E0's future tables lets W1 ship first.
pub fn unconfirmed(ctx: &Ctx, exchange: &str, state: &str, candidates: &[String]) -> String {
    if !matches!(state, "dispatched" | "unresolved") {
        return String::new();
    }
    let text = i18n::t(
        ctx.locale,
        "rules.tile.withdrawal_unconfirmed",
        &[("exchange", Arg::Text(exchange))],
    );
    let candidates = candidates
        .iter()
        .map(|s| escape(&crate::venue::withdrawal::log_safe(s)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "<div class=\"small-info text-danger\">{text}{}</div>",
        if candidates.is_empty() {
            String::new()
        } else {
            format!(" {candidates}")
        }
    )
}

#[derive(Template)]
#[template(path = "rules/tile.html")]
struct Tile<'a> {
    v: &'a Ctx,
    r: &'a Rule,
    csrf: String,
    action: String,
    color: String,
    address: String,
    percentage_input: String,
    fee_input: String,
    min_input: String,
    interval_input: String,
    threshold: String,
    interval: bool,
    info: String,
    empty_addresses: bool,
    no_wallet: String,
    progress: Option<String>,
    status_line: String,
}
pub fn tile(ctx: &Ctx, r: &Rule, list: Option<&Listing>) -> Result<String, WebError> {
    let threshold = r.threshold();
    let progress = if r.status == 1 {
        r.minimum().filter(BigDec::is_positive).map(|min| {
            r.balance
                .as_ref()
                .map(|b| {
                    format::float_round(
                        (&b.div(&min).unwrap_or_else(BigDec::zero) * &BigDec::from_i64(100)).to_f(),
                        1,
                    )
                })
                .map(|p| {
                    if p >= 100.0 {
                        "100".into()
                    } else {
                        format::float_to_s(p)
                    }
                })
                .unwrap_or_else(|| "0".into())
        })
    } else {
        None
    };
    Ok(Tile {
        v: ctx,
        r,
        csrf: ctx.csrf_token(),
        action: ctx.path(&format!("/rules/withdrawals/{}", r.id)),
        color: colors::pill_color(r.asset.color.as_deref()).unwrap_or_default(),
        address: address(r, list, ctx),
        percentage_input: input(r, "withdrawal_percentage", &r.percentage(), false),
        fee_input: input(
            r,
            "max_fee_percentage",
            &r.value("max_fee_percentage", "0.5"),
            threshold == "fee_percentage",
        ),
        min_input: input(
            r,
            "min_amount",
            &r.value("min_amount", "0.1"),
            threshold == "min_amount",
        ),
        interval_input: input(r, "max_interval", &r.text("max_interval"), false),
        interval: !r.text("max_interval").is_empty() || r.status != 1,
        threshold,
        info: info(ctx, r),
        empty_addresses: r.status == 2 && list.is_some_and(|l| l.entries.is_empty()),
        no_wallet: i18n::t(
            ctx.locale,
            "rules.tile.no_wallet_html",
            &[
                ("symbol", Arg::Text(r.asset.symbol())),
                ("exchange", Arg::Text(&r.exchange.name)),
            ],
        ),
        progress,
        status_line: r
            .unconfirmed
            .as_ref()
            .map(|state| {
                let mut line = unconfirmed(ctx, &r.exchange.name, state, &[]);
                if matches!(
                    r.exchange.class.as_str(),
                    "Exchanges::Kraken" | "Exchanges::Gemini"
                ) {
                    line = line.replace(
                        "</div>",
                        &format!(
                            " <code>deltabadger resolve-withdrawal {}</code></div>",
                            r.id
                        ),
                    );
                }
                line
            })
            .unwrap_or_default(),
    }
    .render()?)
}

/// D-RULE-LOG-SECRETS also applies to legacy Rails rows, including HTML after a log prefix.
pub fn safe_message(message: &str) -> String {
    let url = regex::Regex::new(r#"https?://[^\s"'<>]+"#).expect("static URL pattern");
    let no_url = url.replace_all(message, "[redacted URL]");
    let html = regex::Regex::new(r"(?is)<[a-z!/][^>]*>.*").expect("static HTML pattern");
    let no_html = html.replace_all(&no_url, "the venue answered with an HTML page");
    let query = regex::Regex::new(r#"\?[^\s"'<>]*=[^\s"'<>]*"#).expect("static query pattern");
    let no_html = query.replace_all(&no_html, "[redacted query]");
    let parameter =
        regex::Regex::new(r"(?i)(signature|address|api_key|apikey|secret|token)=[^\s&]+")
            .expect("static parameter pattern");
    crate::venue::withdrawal::log_safe(&parameter.replace_all(&no_html, "$1=[redacted]"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn d_rule_log_secrets_scrubs_legacy_failure_text_before_rendering() {
        assert_eq!(
            safe_message("Withdrawal failed: You have insufficient balance."),
            "Withdrawal failed: You have insufficient balance."
        );
        for raw in [
            "Withdrawal failed: GET https://api.example/withdraw?address=wallet&signature=secret",
            "Unexpected error: <html><body>signature=secret</body></html>",
            "Withdrawal failed: signature=secret address=wallet",
        ] {
            let text = safe_message(raw);
            for secret in ["https://", "?address", "wallet", "=secret", "<html>"] {
                assert!(!text.contains(secret), "{text}");
            }
            assert!(text.chars().count() <= 201);
        }
        assert_eq!(safe_message(&"x".repeat(300)).chars().count(), 201);
    }
}
