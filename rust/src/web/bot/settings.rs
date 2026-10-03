//! The settings column of the bot page: the main form of each bot type and every rule form Rails
//! renders under it, switched on or not (bots/dca_multi_assets/_settings, bots/dca_indexes/_settings
//! and bots/settings/*). The forms render; submitting them is not served yet.
//!
//! Each rule is a sentence from the locale file with form controls in its placeholders, so the
//! controls are built here as markup, the way Rails' form helpers build them, and handed to the
//! translation as already-safe arguments.
use super::{start, Asset, Bot, Kind, MIN_COINS};
use crate::web::format::{self, input_value, Num};
use crate::web::i18n::{self, escape, Arg};
use crate::web::layout::Ctx;
use crate::web::{colors, WebError};
use crate::ruby::BigDec;
use askama::Template;
use rusqlite::Connection;
use serde_json::Value;

const AUTOWIDTH: &str = "data-controller=\"autowidth-input\" data-action=\"input-&gt;form--submit-after-delay#submit input-&gt;autowidth-input#resize\" data-form--move-cursor-to-end-target=\"input\"";
const SUBMIT_ON_CHANGE: &str = "data-action=\"change-&gt;form--submit#submit\"";
const NARROW: &str = "style=\"margin-left: 0.125rem; width: 50px;\"";

/// What every form of the column shares.
pub struct Forms<'a> {
    pub ctx: &'a Ctx,
    pub csrf: &'a str,
    pub bot: &'a Bot,
    /// `bot_path(bot)`: every form PATCHes it.
    pub path: String,
    /// BotHelper#hide_balances?: money fields are still posted, but not shown.
    pub hide_balances: bool,
    /// The bot's user's zone name, for the starting-time rule.
    pub time_zone: &'a str,
    /// What the start validation left on the bot, when the status button ran it before these forms.
    pub check: Option<&'a start::Check>,
}

/// config/initializers/inline_form_errors.rb: a field whose attribute has an error is followed by the message, its first letter raised.
fn error_line(message: &str) -> String {
    let mut characters = message.chars();
    let raised: String = characters.next().map(|first| first.to_uppercase().chain(characters).collect()).unwrap_or_default();
    format!("\n             <div class=\"form__info form__info--invalid\">{}</div>\n", escape(&raised))
}

fn disabled(on: bool) -> &'static str {
    if on { " disabled=\"disabled\"" } else { "" }
}

impl Forms<'_> {
    fn rejected(&self, field: &str) -> Option<String> {
        self.check?.rejected.get(field).map(rejected_value)
    }

    fn field_error(&self, field: &str) -> Option<String> {
        let errors: Vec<&str> = self.check?.errors.iter().filter(|e| e.field == field).map(|e| e.message.as_str()).collect();
        (!errors.is_empty()).then(|| errors.join(", "))
    }

    fn key(&self) -> &'static str {
        self.bot.param_key()
    }

    fn t(&self, key: &str, args: &[(&str, Arg)]) -> String {
        i18n::t(self.ctx.locale, key, args)
    }

    fn locked(&self) -> bool {
        self.bot.working()
    }

    /// `form_with model: bot, url: path, method: :patch, class: "widget rule …"` with the class-toggle wiring.
    fn open_rule(&self, active: bool) -> String {
        format!("<form class=\"widget rule {}\" data-controller=\"class-toggle form--submit form--submit-after-delay form--move-cursor-to-end form--html5-validations\" \
                 data-class-toggle-target=\"togglable\" data-class-toggle-toggle-classes-value=\"[&quot;rule--active&quot;,&quot;rule--inactive&quot;]\" data-turbo-stream=\"true\" \
                 action=\"{}\" accept-charset=\"UTF-8\" method=\"post\"><input type=\"hidden\" name=\"_method\" value=\"patch\" /><input type=\"hidden\" name=\"authenticity_token\" value=\"{}\" />",
                if active { "rule--active" } else { "rule--inactive" }, escape(&self.path), escape(self.csrf))
    }

    /// `f.check_box name`: the hidden "0" and the box.
    fn check_box(&self, name: &str, checked: bool, locked: bool) -> String {
        let key = self.key();
        format!("<input name=\"{key}[{name}]\"{locked} type=\"hidden\" value=\"0\" /><input data-action=\"change-&gt;form--submit#submit change-&gt;class-toggle#toggle\"{locked} type=\"checkbox\" value=\"1\"{checked} name=\"{key}[{name}]\" id=\"{key}_{name}\" />",
                locked = disabled(locked), checked = if checked { " checked=\"checked\"" } else { "" })
    }

    /// `f.select name, options, { selected: }, data: { action: … }, disabled: bot.working?`.
    fn select(&self, name: &str, options: &[(String, String)], selected: &str) -> String {
        if let Some(error) = self.field_error(name) { return self.invalid_select(name, options, selected, &error); }
        let key = self.key();
        let options: Vec<String> = options.iter().map(|(label, value)| {
            format!("<option{} value=\"{}\">{}</option>", if value == selected { " selected=\"selected\"" } else { "" }, escape(value), escape(label))
        }).collect();
        format!("<select {SUBMIT_ON_CHANGE}{} name=\"{key}[{name}]\" id=\"{key}_{name}\">{}</select>", disabled(self.locked()), options.join("\n"))
    }

    /// A select whose attribute has an error, as config/initializers/inline_form_errors.rb leaves it:
    /// Nokogiri writes the tag again, with the class it was given last and its boolean attribute bare,
    /// and the message follows.
    fn invalid_select(&self, name: &str, options: &[(String, String)], selected: &str, message: &str) -> String {
        let key = self.key();
        let options: Vec<String> = options.iter().map(|(label, value)| {
            format!("<option{} value=\"{}\">{}</option>", if value == selected { " selected" } else { "" }, escape(value), escape(label))
        }).collect();
        format!("<select {SUBMIT_ON_CHANGE} name=\"{key}[{name}]\" id=\"{key}_{name}\" class=\" is-invalid\">{}</select>{}", options.join("\n"), error_line(message))
    }

    /// The pill a select sits in: the chosen option's words, and the select over them.
    fn pill(&self, words: &str, select: &str) -> String {
        format!("<div class=\"sinput sinput--select {}\">            {}\n            {select}\n</div>", if self.locked() { "sinput--disabled" } else { "" }, escape(words))
    }

    /// `f.number_field name, value:, class: 'sinput numeric-input', step:, size: 2, …`.
    fn number(&self, name: &str, value: Option<&str>, step: &str, extra: &str, locked: bool) -> String {
        let key = self.key();
        let rejected = self.rejected(name);
        let value = rejected.as_deref().or(value).map(|value| format!("value=\"{}\" ", escape(value))).unwrap_or_default();
        let error = self.field_error(name);
        let class = if error.is_some() { " is-invalid" } else { "" };
        let mut out = format!("<input {value}{extra}class=\"sinput numeric-input{class}\" step=\"{step}\" size=\"2\" {AUTOWIDTH} {NARROW}{} type=\"number\" name=\"{key}[{name}]\" id=\"{key}_{name}\" />", disabled(locked));
        if let Some(error) = error { out.push_str(&error_line(&error)); }
        out
    }

    /// BotHelper#amount_field: a number field, or with balances hidden a hidden field that still posts the value.
    fn amount(&self, name: &str, value: Option<&str>, extra: &str) -> String {
        if !self.hide_balances { return self.number(name, value, "any", extra, self.locked()); }
        let key = self.key();
        let rejected = self.rejected(name);
        let value = rejected.as_deref().or(value).map(|value| format!("value=\"{}\" ", escape(value))).unwrap_or_default();
        format!("<input {value}type=\"hidden\" name=\"{key}[{name}]\" id=\"{key}_{name}\" />")
    }

    /// One rule: its switch, and its sentence inside a label for that switch.
    fn rule(&self, active: bool, switch: &str, checked: bool, switch_locked: bool, body: &str) -> String {
        let key = self.key();
        format!("{}\n  <div class=\"toggle-group\">\n    <div class=\"toggle\">\n      {}\n      <div class=\"toggle__style\"></div>\n    </div>\n    <div class=\"toggle-group__info\">\n      \
                 <div class=\"toggle-group__info__label\">\n        <label class=\"conversational conversational--small conversational--disabled\" for=\"{key}_{switch}\">\n{body}</label>      </div>\n    </div>\n  </div>\n</form>",
                self.open_rule(active), self.check_box(switch, checked, switch_locked))
    }

    fn quote(&self) -> &str {
        self.bot.quote_symbol().unwrap_or("USD")
    }

    /// bots/settings/_smart_intervals.html.erb, with its info line.
    pub fn smart_intervals(&self) -> String {
        let bot = self.bot;
        let minimum = start::smart_interval_minimum(bot);
        let stored = bot.number("smart_interval_quote_amount");
        // `[amount, min_amount].max.to_d`: the stored amount unless the floor is above it.
        let value = self.rejected("smart_interval_quote_amount").or_else(|| stored.as_ref().and_then(|amount| input_value(if minimum.value.to_f() > amount.to_f() { &minimum.value } else { amount })));
        let message = start::smart_interval_minimum_message(bot, &minimum, self.ctx.locale)
            .map(|message| format!(" data-html5-range-underflow-message=\"{}\"", escape(&message))).unwrap_or_default();
        let error = self.check.and_then(|check| check.smart_interval_quote_amount.as_deref());
        let key = self.key();
        let shown = value.as_ref().map(|value| format!("value=\"{}\" ", escape(value))).unwrap_or_default();
        let mut field = if self.hide_balances {
            format!("<input {shown}{}type=\"hidden\" name=\"{key}[smart_interval_quote_amount]\" id=\"{key}_smart_interval_quote_amount\" />", if error.is_some() { "class=\" is-invalid\" " } else { "" })
        } else {
            // The number field carries the floor and its message.
            format!("<input {shown}min=\"{}\" class=\"sinput numeric-input{}\" step=\"any\" size=\"2\" {AUTOWIDTH}{message} {NARROW}{} type=\"number\" name=\"{key}[smart_interval_quote_amount]\" id=\"{key}_smart_interval_quote_amount\" />",
                    minimum.value.to_s(), if error.is_some() { " is-invalid" } else { "" }, disabled(self.locked()))
        };
        if let Some(error) = error { field.push_str(&error_line(error)); }
        let sentence = self.t("bot.settings.smart_intervals.sentence_html", &[("amount_html", Arg::Html(&format!("<div>              {field}\n</div>"))), ("quote", Arg::Text(self.quote()))]);
        let active = bot.on("smart_intervaled");
        let mut info = String::new();
        if let (false, true, Some(amount), Some(quote_amount), Some(effective)) = (self.hide_balances, active, stored.filter(Num::is_positive), bot.number("quote_amount").filter(Num::is_positive), bot.effective().filter(|e| e.seconds().is_finite() && (0.0..=super::MAX_SPAN_SECONDS).contains(&e.seconds()))) {
            let decimals = bot.quote_decimals().unwrap_or(2);
            let interval = i18n::text(self.ctx.locale, &format!("bot.{}", bot.text("interval").unwrap_or("")), &[]);
            let words = format::distance_of_time_in_words(effective.seconds(), self.ctx.now, self.ctx.locale);
            info = format!("<small id=\"settings-smart-intervals-info\" class=\"small-info\">\n    {}\n  </small>\n", self.t("bot.settings.smart_intervals.info", &[
                ("quote_amount", Arg::Text(&quote_amount.round(decimals).to_s())), ("quote", Arg::Text(self.quote())), ("interval", Arg::Text(&interval)),
                ("amount", Arg::Text(&amount.round(decimals).to_s())), ("smart_interval", Arg::Text(&words)),
            ]));
        }
        self.rule(active, "smart_intervaled", active, self.locked(), &format!("            {sentence}\n            {info}\n"))
    }

    /// bots/settings/_limit_orders.html.erb. Alpaca never locks limit orders on (Hyperliquid does).
    pub fn limit_orders(&self) -> String {
        let bot = self.bot;
        let active = bot.on("limit_ordered");
        let distance = bot.number("limit_order_pcnt_distance");
        let value = distance.as_ref().and_then(Num::to_d).and_then(|share| input_value(&Num::Dec(&share * &BigDec::from_i64(100))));
        let field = self.number("limit_order_pcnt_distance", value.as_deref(), "any", "", self.locked());
        let sentence = self.t("bot.settings.limit_orders.sentence_html", &[("amount_html", Arg::Html(&format!("<div>            {field}\n</div>")))]);
        let mut info = String::new();
        if active && distance.is_some_and(|d| d.is_positive()) && bot.number("quote_amount").is_some_and(|q| q.is_positive()) {
            info = format!("<small id=\"settings-limit-orders-info\" class=\"small-info\">\n    {}\n  </small>\n", self.t("bot.settings.limit_orders.info", &[
                ("exchange", Arg::Text(&bot.exchange.name)), ("maker_fee", Arg::Text(bot.exchange.maker_fee.as_deref().unwrap_or(""))),
            ]));
        }
        self.rule(active, "limit_ordered", active, self.locked(), &format!("          {sentence}\n            {info}\n"))
    }

    /// bots/settings/_starting_time.html.erb: a weekday or "every day" with a clock time, or a date and time, in the user's zone.
    pub fn starting_time(&self) -> String {
        let (bot, key) = (self.bot, self.key());
        let zone = if self.time_zone.trim().is_empty() { "UTC" } else { self.time_zone };
        let (default_mode, default_time) = format::default_start_time_selection(self.ctx.now, zone).unwrap_or(("monday", "09:30".to_string()));
        let mode = bot.text("start_time_mode").filter(|mode| !mode.trim().is_empty()).unwrap_or(default_mode);
        let options: Vec<(String, String)> = start::MODES.iter().map(|m| (i18n::text(self.ctx.locale, &format!("bot.settings.starting_time.modes.options.{m}"), &[]), m.to_string())).collect();
        // The start validation's one error on the mode: the rule is on and no mode was ever chosen. Only a bot that is not working is validated.
        let select = if self.check.is_some_and(|check| check.start_time_mode) {
            self.invalid_select("start_time_mode", &options, mode, &i18n::text(self.ctx.locale, "activerecord.errors.models.bot.attributes.start_time_mode.inclusion", &[]))
        } else {
            self.select("start_time_mode", &options, mode)
        };
        let pill = format!("<div class=\"sinput sinput--select {}\">\n            {}\n            {select}\n          </div>", if self.locked() { "sinput--disabled" } else { "" },
                           self.t(&format!("bot.settings.starting_time.modes.display.{mode}"), &[]));
        let field = if mode == "date" {
            // The stored start is UTC; the field shows it in the user's zone. Without one: today, at the default time.
            let stored = bot.text("start_at").and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok()).map(|at| format::datetime_local(at.with_timezone(&chrono::Utc), zone));
            let value = stored.unwrap_or_else(|| format!("{}T{default_time}", format::datetime_local(self.ctx.now, zone).split('T').next().unwrap_or("")));
            let error = self.check.and_then(|check| check.start_at);
            format!("<input value=\"{}\" class=\"sinput sinput--datetime{}\" {SUBMIT_ON_CHANGE}{} type=\"datetime-local\" name=\"{key}[start_at]\" id=\"{key}_start_at\" />{}", escape(&value),
                    if error.is_some() { " is-invalid" } else { "" }, disabled(self.locked()),
                    error.map(|kind| error_line(&i18n::text(self.ctx.locale, &format!("activerecord.errors.models.bot.attributes.start_at.{kind}"), &[]))).unwrap_or_default())
        } else {
            let value = bot.text("start_time_of_day").filter(|time| !time.trim().is_empty()).map_or_else(|| if mode == "hour" { "00:00".to_string() } else { default_time.clone() }, str::to_string);
            let error = self.check.is_some_and(|check| check.start_time_of_day);
            format!("<input value=\"{}\" class=\"sinput{}\" {SUBMIT_ON_CHANGE}{} type=\"time\" name=\"{key}[start_time_of_day]\" id=\"{key}_start_time_of_day\" />{}", escape(&value),
                    if error { " is-invalid" } else { "" }, disabled(self.locked()),
                    if error { error_line(&i18n::text(self.ctx.locale, "activerecord.errors.models.bot.attributes.start_time_of_day.invalid", &[])) } else { String::new() })
        };
        let active = bot.start_time_enabled();
        format!("{}\n  <div class=\"toggle-group\">\n    <div class=\"toggle\">\n      {}\n      <div class=\"toggle__style\"></div>\n    </div>\n    <div class=\"toggle-group__info\">\n      <div class=\"toggle-group__info__label\">\n        \
                 <div class=\"conversational conversational--small conversational--disabled\">\n          {pill}\n\n            {field}\n          <span>{}</span>\n        </div>\n      </div>\n    </div>\n  </div>\n</form>",
                self.open_rule(active), self.check_box("start_time_enabled", active, self.locked()), escape(&format::zone_abbreviation(self.ctx.now, zone)))
    }

    /// bots/settings/_amount_limit.html.erb, buying: "Don't spend more than N QUOTE", and what is left of it.
    pub fn amount_limit(&self, c: &Connection) -> Result<String, WebError> {
        let bot = self.bot;
        let active = bot.on("quote_amount_limited");
        let value = bot.number("quote_amount_limit").as_ref().and_then(input_value);
        let field = self.amount("quote_amount_limit", value.as_deref(), "");
        let sentence = self.t("bot.settings.extra_amount_limit.sentence_html", &[("value_html", Arg::Html(&format!("<div>            {field}\n</div>"))), ("quote", Arg::Text(bot.quote_symbol().unwrap_or("")))]);
        let mut info = String::new();
        if !self.hide_balances && active && bot.number("quote_amount_limit").is_some_and(|limit| limit.is_positive()) {
            if let Some(limit) = start::amount_limit(c, bot)? {
                let text = if limit.reached {
                    format!("<span class=\"text-success\">\n          {}\n        </span>", self.t("bot.settings.extra_amount_limit.info.spent", &[]))
                } else {
                    // Rails rounds to `decimals[:quote]`; without a ticker it has none and raises.
                    let decimals = bot.quote_decimals().ok_or_else(|| WebError::Config(format!("bot {}: a spending cap and no ticker to round it by", bot.id)))?;
                    self.t("bot.settings.extra_amount_limit.info.left", &[("amount", Arg::Text(&limit.left.round(decimals).to_s())), ("quote", Arg::Text(bot.quote_symbol().unwrap_or("")))])
                };
                info = format!("<span id=\"settings-amount-limit-info\">\n      {text}\n  </span>\n");
            }
        }
        Ok(self.rule(active, "quote_amount_limited", active, self.locked(), &format!("            {sentence}\n          {info}\n")))
    }

    /// bots/settings/_rebalance.html.erb, switched off (a bot that rebalances is not served yet). Unlike
    /// its siblings it is never locked: the rule is its own leg, independent of the schedule.
    pub fn rebalance(&self) -> String {
        // Bot::Rebalanceable#rebalance_threshold, shown as a percentage.
        let threshold = self.bot.rebalance_threshold();
        let value = threshold.and_then(|share| input_value(&Num::Dec(&share * &BigDec::from_i64(100))));
        let field = self.number("rebalance_threshold", value.as_deref(), "any", "", false);
        let sentence = self.t("bot.settings.rebalance.sentence_html", &[("value_html", Arg::Html(&format!("<div>            {field}\n</div>")))]);
        self.rule(false, "rebalance_enabled", false, false, &format!("          {sentence}\n          \n"))
    }

    /// BotHelper#trigger_mode_select_options and #trigger_mode_for, buying: "Buy only", "Start buying", and for a basket the flip.
    fn trigger_mode(&self, rule: &str) -> (Vec<(String, String)>, &'static str) {
        let tokens: &[&str] = if rule == "price_drop_limit" { &["start", "flip"] } else { &["restrict", "start", "flip"] };
        let options = tokens.iter().map(|token| (i18n::text(self.ctx.locale, &format!("bot.settings.trigger_mode.{token}_buying"), &[]), token.to_string())).collect();
        let current = if matches!(self.bot.text(&format!("{rule}_action")), Some("start_selling" | "start_buying")) { "flip" }
            else if rule == "price_drop_limit" { "start" }
            else if self.bot.text(&format!("{rule}_timing_condition")) == Some("while") { "restrict" } else { "start" };
        (options, current)
    }

    fn mode_pill(&self, rule: &str) -> String {
        let (options, current) = self.trigger_mode(rule);
        self.pill(&i18n::text(self.ctx.locale, &format!("bot.settings.trigger_mode.{current}_buying"), &[]), &self.select(&format!("{rule}_mode"), &options, current))
    }

    /// The stored id of the member a condition watches, as a string for comparing with option values.
    fn watched(&self, rule: &str) -> String {
        self.bot.setting(&format!("{rule}_in_ticker_id")).map(|value| value.as_i64().map_or_else(|| value.as_str().unwrap_or("").to_string(), |id| id.to_string())).unwrap_or_default()
    }

    /// The member a condition names: a select over `options` (label, ticker id) when there are several, else the one's label.
    fn subject(&self, rule: &str, options: &[(String, String)]) -> String {
        if let Some(label) = lone_subject(options) { return label; }
        let watched = self.watched(rule);
        let label = options.iter().find(|(_, id)| *id == watched).map(|(label, _)| label.as_str()).unwrap_or("");
        format!("<div class=\"sinput sinput--select {}\">              {}\n              {}\n</div>", if self.locked() { "sinput--disabled" } else { "" }, escape(label), self.select(&format!("{rule}_in_ticker_id"), options, &watched))
    }

    fn stored(&self, name: &str) -> Option<String> {
        self.bot.number(name).as_ref().and_then(input_value)
    }

    /// The loader an active condition shows until its reading arrives (`bots/settings/_*_info`, cold).
    fn info_loader(&self, rule: &str) -> String {
        format!("<small id=\"settings-{}-info\" class=\"small-info\">\n      <div data-controller=\"broadcast--on-connect\" data-broadcast--on-connect-method-value=\"{rule}_info_update\" \
                 data-broadcast--on-connect-method-args-value=\"{{&quot;bot_id&quot;:{}}}\" class=\"loader--small\" style=\"position: unset; float: left; margin-top: 1rem;\"></div>\n  </small>",
                rule.replace('_', "-"), self.bot.id)
    }

    /// The four conditions of a basket (bots/settings/_price_limit, _price_drop_limit, _moving_average_limit, _indicator_limit).
    /// `members` are BotHelper#base_select_options, `pairs` #ticker_select_options: (label, ticker id), sorted by label.
    pub fn conditions(&self, members: &[(String, String)], pairs: &[(String, String)]) -> String {
        let (bot, locale) = (self.bot, self.ctx.locale);
        let quote = bot.quote_symbol().unwrap_or("");
        let words = |key: String| i18n::text(locale, &key, &[]);
        let boxed = |field: String| format!("<div>            {field}\n</div>");
        let mut out = String::new();

        // Price limit.
        let timing = bot.text("price_limit_timing_condition").unwrap_or("");
        let condition = bot.text("price_limit_value_condition").unwrap_or("");
        let conditions: Vec<(String, String)> = ["above", "below", "between"].iter().filter(|name| **name != "between" || timing == "while")
            .map(|name| (words(format!("bot.settings.extra_price_limit.value_condition.{name}")), name.to_string())).collect();
        let condition_pill = self.pill(&words(format!("bot.settings.extra_price_limit.value_condition.{condition}")), &self.select("price_limit_value_condition", &conditions, condition));
        let price = if condition == "between" {
            let lower = boxed(self.number("price_limit_range_lower_bound", self.stored("price_limit_range_lower_bound").as_deref(), "any", "", self.locked()));
            let upper = boxed(self.number("price_limit_range_upper_bound", self.stored("price_limit_range_upper_bound").as_deref(), "any", "", self.locked()));
            self.t("bot.settings.extra_price_limit.price_limit_range_input_html", &[("lower_bound_value_html", Arg::Html(&lower)), ("upper_bound_value_html", Arg::Html(&upper)), ("quote", Arg::Text(quote))])
        } else {
            let value = boxed(self.number("price_limit", self.stored("price_limit").as_deref(), "any", "", self.locked()));
            self.t("bot.settings.extra_price_limit.price_limit_input_html", &[("value_html", Arg::Html(&value)), ("quote", Arg::Text(quote))])
        };
        let active = bot.on("price_limited");
        let sentence = self.t("bot.settings.extra_price_limit.sentence_html", &[
            ("timing_condition_html", Arg::Html(&self.mode_pill("price_limit"))), ("value_condition_html", Arg::Html(&condition_pill)), ("price_input_html", Arg::Html(&price)),
            ("base_html", Arg::Html(&self.subject("price_limit", members))), ("quote", Arg::Text(quote)), ("count", Arg::Count(members.len() as i64)),
        ]);
        out.push_str(&format!("  \n{}\n", self.rule(active, "price_limited", active, self.locked(), &format!("          {sentence}\n          {}\n", if active { self.info_loader("price_limit") } else { String::new() }))));

        // Price drop limit.
        let window = bot.text("price_drop_limit_time_window_condition").unwrap_or("");
        let windows: Vec<(String, String)> = ["ath", "twenty_four_hours"].iter().map(|name| (words(format!("bot.settings.extra_price_drop_limit.time_window_condition.{name}")), name.to_string())).collect();
        let window_pill = self.pill(&words(format!("bot.settings.extra_price_drop_limit.time_window_condition.{window}")), &self.select("price_drop_limit_time_window_condition", &windows, window));
        let drop = bot.number("price_drop_limit").and_then(|share| share.to_d()).and_then(|share| input_value(&Num::Dec(&share * &BigDec::from_i64(100))));
        let active = bot.on("price_drop_limited");
        let (mode_options, mode) = self.trigger_mode("price_drop_limit");
        let mode_pill = format!("<div class=\"sinput sinput--select {}\">              {}\n              {}\n</div>", if self.locked() { "sinput--disabled" } else { "" },
                                escape(&words(format!("bot.settings.trigger_mode.{mode}_buying"))), self.select("price_drop_limit_mode", &mode_options, mode));
        let sentence = self.t("bot.settings.extra_price_drop_limit.mode_sentence_html", &[
            ("mode_html", Arg::Html(&mode_pill)), ("base_html", Arg::Html(&self.subject("price_drop_limit", members))),
            ("value_html", Arg::Html(&boxed(self.number("price_drop_limit", drop.as_deref(), "any", "", self.locked())))),
            ("time_window_condition_html", Arg::Html(&window_pill)), ("count", Arg::Count(members.len() as i64)),
        ]);
        out.push_str(&format!("  \n{}\n", self.rule(active, "price_drop_limited", active, self.locked(), &format!("          {sentence}\n          {}\n", if active { self.info_loader("price_drop_limit") } else { String::new() }))));

        // Moving average limit.
        const TIMEFRAMES: [&str; 6] = ["one_hour", "four_hours", "one_day", "three_days", "one_week", "one_month"];
        let above_below = |rule: &str| -> String {
            let current = bot.text(&format!("{rule}_value_condition")).unwrap_or("");
            // Both rules take their "above"/"below" option labels from the indicator's keys, as BotHelper does.
            let options: Vec<(String, String)> = ["above", "below"].iter().map(|name| (words(format!("bot.settings.extra_indicator_limit.value_condition.{name}")), name.to_string())).collect();
            let namespace = if rule == "indicator_limit" { "extra_indicator_limit" } else { "extra_moving_average_limit" };
            self.pill(&words(format!("bot.settings.{namespace}.value_condition.{current}")), &self.select(&format!("{rule}_value_condition"), &options, current))
        };
        let timeframe = |rule: &str, namespace: &str| -> String {
            let current = bot.text(&format!("{rule}_in_timeframe")).unwrap_or("");
            let options: Vec<(String, String)> = TIMEFRAMES.iter().map(|name| (words(format!("bot.settings.{namespace}.timeframe.{name}")), name.to_string())).collect();
            format!("<div class=\"sinput sinput--select {}\">              {}\n              {}\n</div>", if self.locked() { "sinput--disabled" } else { "" },
                    escape(&words(format!("bot.settings.{namespace}.timeframe.{current}"))), self.select(&format!("{rule}_in_timeframe"), &options, current))
        };
        let ma_type = bot.text("moving_average_limit_in_ma_type").unwrap_or("");
        let ma_pill = format!("<div class=\"sinput sinput--select {}\">              {}\n              {}\n</div>", if self.locked() { "sinput--disabled" } else { "" }, escape(&ma_type.to_uppercase()),
                              self.select("moving_average_limit_in_ma_type", &[("SMA".to_string(), "sma".to_string()), ("EMA".to_string(), "ema".to_string())], ma_type));
        let period = bot.setting("moving_average_limit_in_period").map(|value| value.as_i64().map_or_else(|| value.to_string(), |n| n.to_string()));
        let watched = self.watched("moving_average_limit");
        // The sentence names the watched pair's base only when there is a choice of pairs.
        let base = if pairs.len() > 1 { bot.tickers.iter().find(|ticker| ticker.id.to_string() == watched).and_then(|ticker| ticker.base_symbol.clone()).unwrap_or_default() } else { String::new() };
        let active = bot.on("moving_average_limited");
        let sentence = self.t("bot.settings.extra_moving_average_limit.sentence_html", &[
            ("timing_condition_html", Arg::Html(&self.mode_pill("moving_average_limit"))), ("base", Arg::Text(&base)), ("value_condition_html", Arg::Html(&above_below("moving_average_limit"))),
            ("ma_html", Arg::Html(&ma_pill)), ("ma_period_html", Arg::Html(&boxed(self.number("moving_average_limit_in_period", period.as_deref(), "1", "", self.locked())))),
            ("ticker_html", Arg::Html(&self.subject("moving_average_limit", pairs))), ("timeframe_html", Arg::Html(&timeframe("moving_average_limit", "extra_moving_average_limit"))),
            ("count", Arg::Count(pairs.len() as i64)),
        ]);
        let info = if active && period.is_some() { self.info_loader("moving_average_limit") } else { String::new() };
        out.push_str(&format!("  \n{}\n", self.rule(active, "moving_average_limited", active, self.locked(), &format!("          {sentence}\n          {info}\n"))));

        // Indicator limit. There is one indicator, so it is named, not chosen.
        let active = bot.on("indicator_limited");
        let sentence = self.t("bot.settings.extra_indicator_limit.sentence_html", &[
            ("timing_condition_html", Arg::Html(&self.mode_pill("indicator_limit"))), ("value_condition_html", Arg::Html(&above_below("indicator_limit"))),
            ("value_html", Arg::Html(&boxed(self.number("indicator_limit", self.stored("indicator_limit").as_deref(), "any", "", self.locked())))),
            ("ticker_html", Arg::Html(&self.subject("indicator_limit", pairs))), ("indicator_html", Arg::Html("RSI")),
            ("timeframe_html", Arg::Html(&timeframe("indicator_limit", "extra_indicator_limit"))), ("count", Arg::Count(pairs.len() as i64)),
        ]);
        let info = if active && bot.setting("indicator_limit").is_some() { self.info_loader("indicator_limit") } else { String::new() };
        out.push_str(&format!("  \n{}\n", self.rule(active, "indicator_limited", active, self.locked(), &format!("          {sentence}\n          {info}\n"))));
        out
    }
}

/// The subject of a condition that has no choice of one, as markup: the label of the only option,
/// escaped, and `None` when there are several. The sentence takes its subject as markup (it is a
/// select when there is a choice), and a label is an asset's symbol out of the database. Rails hands
/// the symbol over raw (`base_html.html_safe`, bots/settings/_price_limit.html.erb and its three
/// siblings); no sentence prints it today, because the form of the
/// sentence for one member names no subject in any locale, and one edited translation would.
pub fn lone_subject(options: &[(String, String)]) -> Option<String> {
    (options.len() <= 1).then(|| options.first().map(|(label, _)| escape(label)).unwrap_or_default())
}

struct Pill {
    symbol: String,
    class: &'static str,
    color: String,
    asset_id: i64,
}

fn pill(asset: &Asset) -> Result<Pill, WebError> {
    Ok(Pill {
        symbol: asset.symbol().to_string(), class: colors::ticker_class(asset.category.as_deref(), asset.color.as_deref()),
        color: colors::pill_color(asset.color.as_deref()).ok_or_else(|| WebError::Config(format!("asset {}: its colour is not a colour", asset.id)))?, asset_id: asset.id,
    })
}

struct Slider {
    pill: Pill,
    /// The weight as a percentage rounded to one decimal, printed as the Float it is ("60.0").
    percent: String,
}

#[derive(Template)]
#[template(path = "bots/settings/_main_basket.html")]
struct BasketMain<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    path: &'a str,
    locked: bool,
    composition_locked: bool,
    weights_locked: bool,
    quote: &'a str,
    quote_asset_id: Option<i64>,
    amount_field: String,
    interval_words: String,
    interval_select: String,
    /// A one-asset basket reads like the pair bot it replaces: the asset in the sentence, no weights.
    lone: Option<Pill>,
    reverse_confirm: Option<String>,
    sliders: Vec<Slider>,
    several: bool,
    balanced: bool,
    total: String,
    can_add: bool,
    add_path: String,
}

struct Coin {
    color: String,
    market_cap: String,
    asset_id: i64,
    symbol: String,
    visible: bool,
}

#[derive(Template)]
#[template(path = "bots/settings/_main_index.html")]
struct IndexMain<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    path: &'a str,
    bot_id: i64,
    locked: bool,
    quote: &'a str,
    quote_asset_id: Option<i64>,
    amount_field: String,
    interval_words: String,
    interval_select: String,
    index_name: String,
    num_coins: i64,
    max_coins: i64,
    num_coins_progress: i64,
    flattening: String,
    flattening_progress: i64,
    coins: Vec<Coin>,
    fewer_note: String,
}

/// `[t("bot.hour"), "hour"], …`: BotHelper#bot_intervals_select_options.
fn intervals(locale: &str) -> Vec<(String, String)> {
    ["hour", "day", "week", "month"].iter().map(|name| (i18n::text(locale, &format!("bot.{name}"), &[]), name.to_string())).collect()
}

/// Bots::DcaIndex#current_index_preview with the Deltabadger provider: the index's members that
/// trade here in the bot's quote, in index order, each with the market cap it is weighted by
/// (the asset's own, else the weight the index carries for it).
fn index_preview(c: &Connection, bot: &Bot) -> Result<Vec<Coin>, WebError> {
    let Some(index) = bot.index.as_ref() else { return Ok(vec![]) };
    if bot.setting("quote_asset_id").is_none() { return Ok(vec![]); }
    let limit = bot.bounded_universe_size().unwrap_or(250) as usize;
    let mut coins = vec![];
    let num_coins = shown_coins(bot);
    // Looked up once: a member is asked for among every listing of the venue.
    let listed: std::collections::HashSet<i64> = bot.tickers.iter().map(|ticker| ticker.base_asset_id).collect();
    for external_id in index.top_coins.iter().take(limit) {
        if coins.len() as i64 >= bot.max_coins() { break; }
        let Some(asset) = Asset::find_by_external_id(c, external_id)? else { continue };
        // MarketData.get_top_coins: a real market cap when there is one, else the index's weight; a member with neither is skipped.
        let own = asset.market_cap.map_or(0.0, |cap| cap as f64);
        let cap = if own > 0.0 { own } else { index.weights.get(external_id).and_then(Value::as_f64).unwrap_or(0.0) };
        if cap <= 0.0 { continue; }
        if !listed.contains(&asset.id) { continue; }
        coins.push(Coin {
            color: colors::pill_color(asset.color.as_deref()).ok_or_else(|| WebError::Config(format!("asset {}: its colour is not a colour", asset.id)))?,
            market_cap: format::float_to_s(cap), asset_id: asset.id, symbol: asset.symbol().to_string(), visible: (coins.len() as i64) < num_coins,
        });
    }
    Ok(coins)
}

/// The count the index form's slider is drawn at: the stored one, held within what the index offers.
fn shown_coins(bot: &Bot) -> i64 {
    bot.effective_num_coins().unwrap_or(10).max(MIN_COINS)
}

/// `div#settings`: the whole column of one bot.
pub fn column(c: &Connection, forms: &Forms) -> Result<String, WebError> {
    let (bot, ctx) = (forms.bot, forms.ctx);
    let rejected_quote = bot.setting("quote_asset_id").map(rejected_value).unwrap_or_default();
    let quote = bot.quote_symbol().unwrap_or(&rejected_quote);
    let quote_asset_id = bot.setting("quote_asset_id").and_then(Value::as_i64);
    let amount = forms.rejected("quote_amount").or_else(|| bot.number("quote_amount").as_ref().and_then(input_value)).unwrap_or_else(|| "100".to_string());
    let mut amount_field = if forms.hide_balances { forms.amount("quote_amount", Some(&amount), "") } else {
        format!("<input value=\"{}\" class=\"numeric-input\" step=\"any\" size=\"2\" {AUTOWIDTH}{} type=\"number\" name=\"{key}[quote_amount]\" id=\"{key}_quote_amount\" />", escape(&amount), disabled(bot.working()), key = bot.param_key())
    };
    if let Some(error) = forms.field_error("quote_amount") {
        amount_field = amount_field.replace("class=\"numeric-input\"", "class=\"numeric-input is-invalid\"");
        amount_field.push_str(&error_line(&error));
    }
    let interval_value = bot.setting("interval").map(rejected_value).unwrap_or_default();
    let interval = interval_value.as_str();
    let mut options = intervals(ctx.locale);
    let interval_words = if options.iter().any(|(_, value)| value == interval) {
        i18n::text(ctx.locale, &format!("bot.{interval}"), &[])
    } else {
        options.push((interval_value.clone(), interval_value.clone()));
        interval_value.clone()
    };
    let interval_select = forms.select("interval", &options, interval);
    let mut out = String::from("<div id=\"settings\" class=\"column gap-1\">\n  ");
    match bot.kind {
        Kind::Basket => {
            let lone = if bot.base_assets.len() == 1 { bot.base_assets.first().map(pill).transpose()? } else { None };
            let sliders = if lone.is_some() { vec![] } else {
                bot.base_assets.iter().map(|asset| {
                    let weight = bot.allocations().iter().find(|(id, _)| *id == asset.id).map_or(0.0, |(_, weight)| *weight);
                    Ok(Slider { pill: pill(asset)?, percent: format::float_to_s(format::float_round(weight * 100.0, 1)) })
                }).collect::<Result<Vec<_>, WebError>>()?
            };
            let total = format::number_with_precision(&Num::Float(bot.allocations_total() * 100.0), 1, false).unwrap_or_default();
            let composition_locked = bot.working() || bot.rebalance_pending();
            out.push_str(&BasketMain {
                v: ctx, csrf: forms.csrf, path: &forms.path, locked: bot.working(), composition_locked,
                weights_locked: composition_locked || bot.text("weighting") == Some("market_cap"), quote, quote_asset_id, amount_field, interval_words, interval_select, lone,
                reverse_confirm: bot.has_regular_waiting_orders.then(|| i18n::text(ctx.locale, "bot.reverse_confirm", &[])),
                several: bot.base_assets.len() > 1, sliders, balanced: bot.allocations_balanced(), total,
                can_add: !composition_locked && bot.base_assets.len() < super::MAX_ASSETS,
                add_path: format!("{}?asset_field=add_asset_id", ctx.path(&format!("/bots/{}/asset_search/edit", bot.id))),
            }.render()?);
            out.push('\n');
            // The market-cap rule is offered only where every member has a market cap to be weighted by.
            if !bot.one_asset() && (bot.text("weighting") == Some("market_cap") || (!bot.base_assets.is_empty() && bot.base_assets.iter().all(|asset| asset.market_cap.is_some_and(|cap| cap >= 1)))) {
                out.push_str(&market_cap_rule(forms));
            }
            out.push_str(&format!("    {}\n  {}\n  {}\n", forms.smart_intervals(), forms.limit_orders(), forms.starting_time()));
            // BotHelper#base_select_options and #ticker_select_options.
            let mut members: Vec<(String, String)> = bot.composition_tickers().iter().map(|ticker| (ticker.base_symbol.clone().unwrap_or_default(), ticker.id.to_string())).collect();
            members.sort_by(|a, b| a.0.cmp(&b.0));
            let mut pairs: Vec<(String, String)> = bot.tickers.iter().map(|ticker| (format!("{}{}", ticker.base_symbol.as_deref().unwrap_or(""), ticker.quote_symbol.as_deref().unwrap_or("")), ticker.id.to_string())).collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            out.push_str(&forms.conditions(&members, &pairs));
            out.push_str(&format!("    {}\n", forms.amount_limit(c)?));
            if !bot.one_asset() { out.push_str(&format!("    {}\n", forms.rebalance())); }
        }
        Kind::Index => {
            let coins = index_preview(c, bot)?;
            let max_coins = if coins.is_empty() { bot.max_coins() } else { bot.max_coins().min(coins.len() as i64) };
            let num_coins = shown_coins(bot).min(max_coins);
            let flattening = bot.number("allocation_flattening").unwrap_or(Num::Int(0));
            let fewer_note = i18n::text(ctx.locale, "bot.dca_index.fewer_coins_available", &[
                ("available", Arg::Text(&coins.len().to_string())), ("requested", Arg::Text(&num_coins.to_string())), ("exchange_name", Arg::Text(&bot.exchange.name)),
            ]);
            out.push_str(&IndexMain {
                v: ctx, csrf: forms.csrf, path: &forms.path, bot_id: bot.id, locked: bot.working(), quote, quote_asset_id, amount_field, interval_words, interval_select,
                index_name: bot.display_index_name().unwrap_or_else(|| i18n::text(ctx.locale, "bot.dca_index.setup.pick_index.top_coins", &[])),
                num_coins, max_coins,
                num_coins_progress: if max_coins > MIN_COINS { ((num_coins - MIN_COINS) as f64 / (max_coins - MIN_COINS) as f64 * 100.0).round() as i64 } else { 100 },
                flattening_progress: (flattening.to_f() * 100.0).round() as i64, flattening: flattening.to_s(),
                coins: coins.into_iter().enumerate().map(|(position, coin)| Coin { visible: (position as i64) < num_coins, ..coin }).collect(), fewer_note,
            }.render()?);
            out.push_str(&format!("\n  {}\n  {}\n  {}\n  {}\n", forms.smart_intervals(), forms.limit_orders(), forms.starting_time(), forms.rebalance()));
        }
    }
    out.push_str("</div>");
    Ok(out)
}

/// bots/dca_multi_assets/settings/_marketcap_allocation.html.erb, switched off (market-cap weights are not served yet).
fn market_cap_rule(forms: &Forms) -> String {
    let key = forms.key();
    format!("<form class=\"widget main-rule main-rule--multi-asset {active}\" data-controller=\"class-toggle form--submit\" data-class-toggle-target=\"togglable\" \
             data-class-toggle-toggle-classes-value=\"[&quot;rule--active&quot;,&quot;rule--inactive&quot;]\" data-turbo-stream=\"true\" action=\"{path}\" accept-charset=\"UTF-8\" method=\"post\">\
             <input type=\"hidden\" name=\"_method\" value=\"patch\" /><input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" />\n  <div class=\"toggle-group\">\n    <div class=\"toggle\">\n      \
             <input name=\"{key}[weighting]\"{locked} type=\"hidden\" value=\"manual\" /><input data-action=\"change-&gt;form--submit#submit change-&gt;class-toggle#toggle\"{locked} type=\"checkbox\" value=\"market_cap\"{checked} name=\"{key}[weighting]\" id=\"{key}_weighting\" />\n      \
             <div class=\"toggle__style\"></div>\n    </div>\n    <div class=\"toggle-group__info\" data-controller=\"class-toggle\" data-class-toggle-toggle-classes-value='[\"hidden\"]'>\n      <div class=\"toggle-group__info__label\">\n        \
             <label class=\"conversational conversational--small\" for=\"{key}_weighting\">{label}</label>\n        <div class=\"tooltip-info-icon\" data-controller=\"tooltip\" data-action=\"click->tooltip#toggle click->class-toggle#toggle\">\n          {info}\n          {filled}\n        </div>\n      </div>\n      \
             <div class=\"bot-option-info hidden\" data-class-toggle-target=\"togglable\">\n        {text}\n      </div>\n    </div>\n  </div>\n</form>\n",
            path = escape(&forms.path), csrf = escape(forms.csrf), locked = disabled(forms.locked() || forms.bot.rebalance_pending()), label = forms.t("bot.utils.mkt_cap_adjusted_html", &[]),
            active = if forms.bot.text("weighting") == Some("market_cap") { "rule--active" } else { "rule--inactive" },
            checked = if forms.bot.text("weighting") == Some("market_cap") { " checked=\"checked\"" } else { "" },
            info = include_str!("../../../templates/svg/_24x24_info.html"), filled = include_str!("../../../templates/svg/_24x24_info_filled.html"), text = forms.t("bot.utils.mkt_cap_adjusted_info_html", &[]))
}

/// A rejected proposal is rendered directly; it never goes back through Bot::unrendered or the
/// persisted-row refusal. Dynamic arithmetic below already uses fallible interval/number reads.
pub fn draft_column(c: &Connection, ctx: &Ctx, csrf: &str, draft: &super::draft::Draft, time_zone: &str, hide_balances: bool) -> Result<String, WebError> {
    let check = draft.check();
    let forms = Forms { ctx, csrf, bot: &draft.candidate, path: crate::web::locale::path(ctx.locale, &format!("/bots/{}", draft.candidate.id)),
        hide_balances, time_zone, check: Some(&check) };
    let mut html = column(c, &forms)?;
    for field in ["quote_asset_id", "exchange"] {
        if let Some(error) = forms.field_error(field) {
            let value = if field == "exchange" { draft.submitted_exchange_id.as_ref() } else { draft.candidate.setting(field) };
            let value = value.map(rejected_value).unwrap_or_default();
            let content = format!("<div class=\"form__info form__info--invalid\" data-field=\"{field}\">{}: {}</div>", escape(&value), escape(&error));
            // Insert before the outer column closes, keeping the regular settings stream target.
            if let Some(end) = html.rfind("</div>") { html.insert_str(end, &content); }
        }
    }
    Ok(html)
}

fn rejected_value(value: &Value) -> String {
    let text = value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string());
    text.chars().take(128).collect()
}
