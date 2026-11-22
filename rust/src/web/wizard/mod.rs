//! The new-bot wizard: BotsController#new (the bot-type chooser) and the order switch
//! (Bots::DcaSingleAssets::OrdersController), with the step order they share (Bots::Wizard::StepOrder and
//! Bots::Wizard::Navigable), and its steps: assets (`assets`), exchange (`exchanges`, single and multi namespaces), stock broker
//! (`brokers`), API key (`keys`, both namespaces) and quote (`spendable`), whose POST saves the bot (`create`). The
//! conversational sentence, the sheet with its track (D-WIZARD-SHEET) and the stock activation notice the steps share are here. The index wizard
//! (Bots::DcaIndexes::*) is `index`, its save `index_create`; it shares the exchange table, the key step and the quote search.
//! The signal wizard (Bots::Signals::*) is `signals`, with the key step's fourth namespace.
//!
//! The wizard's state is `session[:bot_config]` (web::session), a JSON object with string keys as 2.x's
//! cookie serializer holds it.
use super::layout::{self, Ctx, Page};
use super::shell::{self, Shell};
use super::{auth, colors, App, WebError};

pub mod assets;
pub mod brokers;
pub mod create;
pub mod exchanges;
pub mod index;
pub mod index_create;
pub mod keys;
pub mod signals;
pub mod spendable;
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::Response;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step { Currencies, Assets, Exchange, Api, Spendable }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotType { Single, Multi }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant { AssetFirst, ExchangeFirst }

/// A session key path, relative to `session[:bot_config]`.
pub type Key = &'static [&'static str];
pub const EXCHANGE_KEY: Key = &["exchange_id"];
pub const BASE_KEY: Key = &["settings", "base_asset_id"];
pub const BASE0_KEY: Key = &["settings", "base0_asset_id"];
pub const BASE1_KEY: Key = &["settings", "base1_asset_id"];
pub const BASE_IDS_KEY: Key = &["settings", "base_asset_ids"];
pub const QUOTE_KEY: Key = &["settings", "quote_asset_id"];
/// BASE0/BASE1 stay as a scrub of sessions left open mid-dual-wizard.
pub const ALL_WIZARD_KEYS: [Key; 6] = [EXCHANGE_KEY, BASE_KEY, BASE0_KEY, BASE1_KEY, BASE_IDS_KEY, QUOTE_KEY];

/// Bots::Wizard::StepOrder: the step sequence of one (bot type, variant), and what each step owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepOrder { pub steps: &'static [Step] }

impl StepOrder {
    pub fn new(bot_type: BotType, variant: Variant) -> Self {
        use Step::*;
        let steps: &'static [Step] = match (bot_type, variant) {
            (BotType::Single, Variant::AssetFirst) => &[Currencies, Exchange, Api, Spendable],
            (BotType::Single, Variant::ExchangeFirst) => &[Exchange, Api, Currencies, Spendable],
            (BotType::Multi, Variant::AssetFirst) => &[Assets, Exchange, Api, Spendable],
            (BotType::Multi, Variant::ExchangeFirst) => &[Exchange, Api, Assets, Spendable],
        };
        Self { steps }
    }

    pub fn first(&self) -> Step { self.steps[0] }

    pub fn next_after(&self, step: Step) -> Option<Step> {
        let index = self.steps.iter().position(|s| *s == step)?;
        self.steps.get(index + 1).copied()
    }

    /// The session keys a step writes. :api lives in the database, so it owns none.
    pub fn owned_keys(step: Step) -> &'static [Key] {
        match step {
            Step::Currencies | Step::Assets => &[BASE_IDS_KEY],
            Step::Exchange => &[EXCHANGE_KEY],
            Step::Spendable => &[QUOTE_KEY],
            Step::Api => &[],
        }
    }

    /// What (re-)committing `step` clears: every wizard key except the earlier steps' and, unless `step` is the
    /// asset step, the chosen assets (sticky: only an asset step clears them).
    pub fn reset_keys(&self, step: Step) -> Vec<Key> {
        let earlier = self.steps.iter().position(|s| *s == step).map_or(&[][..], |index| &self.steps[..index]);
        let mut preserved: Vec<Key> = earlier.iter().flat_map(|s| Self::owned_keys(*s).iter().copied()).collect();
        if !matches!(step, Step::Currencies | Step::Assets) {
            preserved.extend(self.steps.iter().filter(|s| matches!(s, Step::Currencies | Step::Assets)).flat_map(|s| Self::owned_keys(*s).iter().copied()));
            preserved.push(BASE_KEY);
        }
        ALL_WIZARD_KEYS.into_iter().filter(|key| !preserved.contains(key)).collect()
    }
}

/// `session.dig(:bot_config, *path)`.
pub fn dig(config: Option<&Value>, path: Key) -> Option<&Value> {
    path.iter().try_fold(config?, |value, key| value.get(*key))
}

/// Object#present? on a decoded JSON value.
fn present(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null | Value::Bool(false)) => false,
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(entries)) => !entries.is_empty(),
        Some(Value::Bool(true) | Value::Number(_)) => true,
    }
}

/// The index keys a basket write clears (D-INDEX-OR-BASKET), `allocation_flattening` among them: the default a refused index save
/// leaves behind.
pub const INDEX_KEYS: [Key; 5] = [&["settings", "index_type"], &["settings", "index_category_id"], &["settings", "index_name"],
                                  &["settings", "index_name_prefix"], &["settings", "allocation_flattening"]];

/// D-INDEX-IN-ASSETS: an index is chosen on the Assets step.
pub fn index_chosen(config: Option<&Value>) -> bool { present(dig(config, &["settings", "index_type"])) }

/// D-INDEX-FRESH-START: asset-first with neither a basket nor an index chosen, where a venue left behind is neither shown nor
/// searched against.
pub fn fresh_start(config: Option<&Value>, basket_empty: bool) -> bool {
    current_variant(config) == Variant::AssetFirst && basket_empty && !index_chosen(config)
}

/// The Assets step's Index view, where every index step's gate sends the browser.
pub const INDEX_VIEW: &str = "/bots/dca_single_assets/pick_buyable_asset/new?kind=index";
const ASSETS_NEW: &str = "/bots/dca_single_assets/pick_buyable_asset/new";

#[derive(Template)]
#[template(path = "wizard/_kind_switch.html")]
struct KindSwitch { label: String, options: [(String, String, bool); 2] }

/// The Asset / Index switch in the Assets step's search row (D-INDEX-IN-ASSETS): two links to the step, the shown one current.
pub fn kind_switch(ctx: &Ctx, index: bool) -> Result<String, WebError> {
    let option = |kind: &str, on: bool| (ctx.t(&format!("bot.setup.kind.{kind}")), ctx.path(&format!("{ASSETS_NEW}?kind={kind}")), on);
    Ok(KindSwitch { label: ctx.t("bot.setup.progress_steps.assets"), options: [option("asset", !index), option("index", index)] }.render()?)
}

/// The pane's search box (`wizard/_search.html`).
pub fn search_box(frame: &str, url: &str, placeholder: &str) -> Result<String, WebError> {
    Ok(Search { frame, url, placeholder }.render()?)
}

/// Where an index step is: the Assets step's Index view, the venue, its key, the quote.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum At { Assets, Exchange, Key, Quote }

/// The index track (D-INDEX-IN-ASSETS): Exchange, Assets, Currency in the order in force, done, current or upcoming by place.
/// Exchange shows the venue's name once chosen (not on the exchange step), Assets the index's name; an item other than the
/// current one whose value is chosen goes back to its step.
pub fn index_track(ctx: &Ctx, csrf: &str, config: Option<&Value>, at: At, index: &str, venue: Option<String>) -> Result<String, WebError> {
    let order: [usize; 3] = if current_variant(config) == Variant::AssetFirst { [1, 0, 2] } else { [0, 1, 2] };
    let current = match at { At::Assets => 1, At::Exchange | At::Key => 0, At::Quote => 2 };
    let current_place = order.iter().position(|item| *item == current).unwrap_or(0);
    let venue = venue.filter(|_| at != At::Exchange);
    let mut items = vec![];
    for (place, item) in order.into_iter().enumerate() {
        let state = match place.cmp(&current_place) { std::cmp::Ordering::Less => "done", std::cmp::Ordering::Equal => "current", _ => "upcoming" };
        let (label, back) = match item {
            0 => (venue.clone().unwrap_or_else(|| ctx.t("bot.setup.progress_steps.exchange")), venue.is_some().then(|| ctx.path("/bots/dca_indexes/pick_exchange/new"))),
            1 => (index.to_string(), Some(ctx.path(ASSETS_NEW))),
            _ => (ctx.t("bot.setup.progress_steps.spendable"), None),
        };
        let action = match back { Some(url) if item != current => Action::get(url, true), _ => Action::plain() };
        items.push(Item { state, label, chips: vec![], more: 0, stack_class: "", action });
    }
    Ok(Track { items, csrf }.render()?)
}

/// Navigable#current_variant: exchange-first unless the order switch chose asset-first (it writes only those two).
pub fn current_variant(config: Option<&Value>) -> Variant {
    if dig(config, &["flow"]).and_then(Value::as_str) == Some("asset_first") { Variant::AssetFirst } else { Variant::ExchangeFirst }
}

/// Navigable#step_complete?. The API key step is complete when the user's trading key for the chosen venue is
/// correct, which the database knows, not the session.
pub fn step_complete(c: &Connection, user_id: i64, config: Option<&Value>, step: Step) -> rusqlite::Result<bool> {
    match step {
        Step::Api => {
            // `user.api_keys.find_by(exchange_id:, key_type: :trading)`, then `correct?`: the picked account's (#710 7a §2); an
            // Exchanges tile not yet given a key always asks.
            if untargeted(config) { return Ok(false) }
            let Some(account) = account(c, user_id, config)? else { return Ok(false) };
            let status: Option<i64> = c.query_row(&format!("SELECT status FROM api_keys k WHERE {} AND key_type = 0 ORDER BY id LIMIT 1", account.keys("k")),
                                                  [], |r| r.get(0)).optional()?;
            Ok(status == Some(1))
        }
        // `Array(...).any?`: one asset is a basket too; only an empty list leaves the step incomplete.
        Step::Assets => Ok(match dig(config, BASE_IDS_KEY) {
            None | Some(Value::Null | Value::Bool(false)) => false,
            Some(Value::Array(ids)) => ids.iter().any(|id| !matches!(id, Value::Null | Value::Bool(false))),
            Some(Value::Object(entries)) => !entries.is_empty(),
            Some(_) => true,
        }),
        _ => Ok(StepOrder::owned_keys(step).iter().all(|key| present(dig(config, key)))),
    }
}

/// Navigable#first_incomplete.
pub fn first_incomplete(c: &Connection, user_id: i64, config: Option<&Value>, order: &StepOrder) -> rusqlite::Result<Step> {
    for step in order.steps {
        if !step_complete(c, user_id, config, *step)? { return Ok(*step) }
    }
    Ok(order.steps[order.steps.len() - 1])
}

/// Navigable#prerequisite_redirect_path, as the step to go back to: an earlier step that is not complete.
pub fn prerequisite(c: &Connection, user_id: i64, config: Option<&Value>, order: &StepOrder, current: Step) -> rusqlite::Result<Option<Step>> {
    let target = first_incomplete(c, user_id, config, order)?;
    let at = |step: Step| order.steps.iter().position(|s| *s == step);
    Ok((at(target) < at(current)).then_some(target))
}

/// The controller's first_incomplete_after: the first step after `step` that is not complete, or the last.
pub fn first_incomplete_after(c: &Connection, user_id: i64, config: Option<&Value>, order: &StepOrder, step: Step) -> rusqlite::Result<Step> {
    let rest = order.steps.iter().position(|s| *s == step).map_or(&[][..], |index| &order.steps[index + 1..]);
    for step in rest {
        if !step_complete(c, user_id, config, *step)? { return Ok(*step) }
    }
    Ok(order.steps[order.steps.len() - 1])
}

fn delete_path(config: &mut Value, key: Key) {
    let (leaf, parents) = key.split_last().expect("a key path is never empty");
    if let Some(container) = parents.iter().try_fold(&mut *config, |value, key| value.get_mut(*key)).and_then(Value::as_object_mut) {
        container.remove(*leaf);
    }
}

/// The asset step's `write_ids` scrub: every wizard key but the exchange.
pub fn reset_keys_but_exchange(config: &mut Value) {
    for key in ALL_WIZARD_KEYS.into_iter().filter(|key| *key != EXCHANGE_KEY) { delete_path(config, key); }
}

/// `Asset.find_by(external_id: 'usd')`.
pub fn usd_asset(c: &Connection) -> rusqlite::Result<Option<i64>> {
    c.query_row("SELECT id FROM assets WHERE external_id = 'usd' ORDER BY id LIMIT 1", [], |r| r.get(0)).optional()
}

/// Navigable#reset_downstream!: clear what `current` and the steps after it own. An emptied parent stays.
pub fn reset_downstream(config: &mut Value, order: &StepOrder, current: Step) {
    for key in order.reset_keys(current) { delete_path(config, key); }
}

impl Step {
    /// The step's key, as the views and i18n name it.
    pub fn key(self) -> &'static str {
        match self { Step::Currencies => "currencies", Step::Assets => "assets", Step::Exchange => "exchange", Step::Api => "api", Step::Spendable => "spendable" }
    }
}

/// 2.x's `to_i` on a decoded session value: an Integer as it is, a String read as 2.x reads it, nothing else a number.
pub fn to_i(value: &Value) -> i64 {
    match value {
        Value::Number(number) => number.as_i64().unwrap_or_else(|| number.as_f64().map_or(0, |f| f as i64)),
        Value::String(text) => crate::web::bot::search::to_i(text),
        _ => 0,
    }
}

/// `Array(value)`: a list as it is, nothing as none, anything else as one.
fn array(value: Option<&Value>) -> Vec<Value> {
    match value {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(items)) => items.clone(),
        Some(other) => vec![other.clone()],
    }
}

/// `bot.exchange_id` of a bot built from the session: ActiveRecord's integer cast of `exchange_id` (a blank string is nil).
pub fn exchange_id(config: Option<&Value>) -> Option<i64> {
    match dig(config, EXCHANGE_KEY)? {
        Value::String(text) if text.trim().is_empty() => None,
        value @ (Value::String(_) | Value::Number(_)) => Some(to_i(value)),
        _ => None,
    }
}

/// #710 7a §2: how the venue was picked. `Account`: a Connected tile (its account); `Untargeted`: a Brokers/Exchanges tile,
/// whose key step always asks and saves where the key routes; `Legacy`: a 2.x form, the exchange's number 1 as before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picked { Account(i64), Untargeted, Legacy }

/// The pick a venue step's form posted for `exchange` (`account_id` on a Connected tile, `untargeted` on the form). None: the
/// posted account is not a live account of this user and exchange (a 404).
pub fn picked(c: &Connection, ctx: &crate::web::layout::Ctx, user: i64, exchange: i64) -> rusqlite::Result<Option<Picked>> {
    let posted = ctx.params.query("account_id").or_else(|| ctx.params.form("account_id"));
    if let Some(posted) = posted {
        return Ok(match crate::identity::pick(c, user, exchange, Some(posted.parse().unwrap_or(0)))? {
            crate::identity::Pick::One(account) => account.id.map(Picked::Account),
            _ => None,
        });
    }
    Ok(Some(if ctx.params.form("untargeted").is_some() { Picked::Untargeted } else { Picked::Legacy }))
}

/// The session keeps the pick: `account_id`, or `untargeted`, neither for a 2.x form.
pub fn keep_pick(config: &mut Value, picked: Picked) {
    if let Some(map) = config.as_object_mut() { map.remove("account_id"); map.remove("untargeted"); }
    match picked {
        Picked::Account(id) => config["account_id"] = json!(id),
        Picked::Untargeted => config["untargeted"] = json!(true),
        Picked::Legacy => {}
    }
}

/// What a wizard form posts to say which pick it rendered (#710 7a §2): `?exchange_id=E&account_id=N` or
/// `?exchange_id=E&untargeted=1`; nothing for a 2.x session (no pick kept), whose forms stay as they were.
pub fn bound_query(config: Option<&Value>) -> String {
    let Some(exchange) = exchange_id(config) else { return String::new() };
    match dig(config, &["account_id"]).and_then(Value::as_i64) {
        Some(account) => format!("?exchange_id={exchange}&account_id={account}"),
        None if untargeted(config) => format!("?exchange_id={exchange}&untargeted=1"),
        None => String::new(),
    }
}

/// Whether a wizard form's post still matches the session (another tab may have picked another venue or account since): a
/// form that names a pick, only that same pick; one that names nothing (a 2.x one, rendered for no pick: a 2.x session, a
/// single-account venue, the sole-broker shortcut) only while the session still has no pick (rule H5: every pick, the
/// automatic ones included, binds the forms rendered for it, and a single-account install's forms keep their bytes).
pub fn still_bound(ctx: &crate::web::layout::Ctx, config: Option<&Value>) -> bool {
    let named = |name: &str| ctx.params.query(name);
    let Some(exchange) = named("exchange_id") else { return dig(config, &["account_id"]).is_none() && !untargeted(config) };
    let account = named("account_id").map(str::to_string);
    Some(exchange.parse::<i64>().unwrap_or(0)) == exchange_id(config)
        && account == dig(config, &["account_id"]).and_then(Value::as_i64).map(|a| a.to_string())
        && named("untargeted").is_some() == untargeted(config)
}

/// Whether the venue step was an Exchanges tile not yet given a key (§2: its key step always asks).
pub fn untargeted(config: Option<&Value>) -> bool { dig(config, &["untargeted"]).and_then(Value::as_bool) == Some(true) }

/// The account the bot built from the session trades on: the picked one (or the one its key was saved on), else its exchange's
/// number 1. None: no exchange, or a picked account no longer a live account of this user and exchange.
pub fn account(c: &Connection, user: i64, config: Option<&Value>) -> rusqlite::Result<Option<crate::identity::Account>> {
    let Some(exchange) = exchange_id(config) else { return Ok(None) };
    match dig(config, &["account_id"]).and_then(Value::as_i64) {
        Some(id) => Ok(match crate::identity::pick(c, user, exchange, Some(id))? { crate::identity::Pick::One(account) => Some(account), _ => None }),
        None => Ok(Some(crate::identity::Account::first(c, user, exchange)?)),
    }
}

/// The session named an account that is no longer a live account of this user and exchange (`bot_account`).
pub struct Gone;
/// `account` for the step that makes the bot (#710 7a §2): Err(Gone) when the session's pick was removed or merged since
/// (another tab): the form is stale, and the bot never falls to number 1.
pub fn bot_account(c: &Connection, user: i64, config: Option<&Value>) -> rusqlite::Result<Result<Option<crate::identity::Account>, Gone>> {
    let account = account(c, user, config)?;
    Ok(if account.is_none() && exchange_id(config).is_some() { Err(Gone) } else { Ok(account) })
}

/// Bots::DcaMultiAsset#base_asset_ids of a basket built from the session: `Array(settings['base_asset_ids']).map(&:to_i)`.
pub fn basket_ids(config: Option<&Value>) -> Vec<i64> {
    array(dig(config, BASE_IDS_KEY)).iter().map(to_i).collect()
}

/// The asset step's chosen ids, from whichever shape the session holds (`base_asset_ids.presence || base_asset_id`), without
/// blanks and duplicates; `chips` then drops what is no longer an Asset row.
pub fn chosen_ids(config: Option<&Value>) -> Vec<i64> {
    let ids = dig(config, BASE_IDS_KEY).filter(|ids| present(Some(ids))).or_else(|| dig(config, BASE_KEY));
    let mut chosen: Vec<i64> = vec![];
    for id in array(ids).iter().map(to_i).filter(|id| *id != 0) {
        if !chosen.contains(&id) { chosen.push(id); }
    }
    chosen
}

/// An asset as a ticker pill shows it.
pub struct Chip {
    pub id: i64,
    pub symbol: String,
    pub name: String,
    pub class: &'static str,
    pub pill: String,
    /// The asset's logo, as a search result draws it.
    pub logo: String,
}

/// The Assets of `ids` in that order, skipping ids that are no row (`ids.filter_map { by_id[_] }`).
pub fn chips(c: &Connection, ids: &[i64]) -> Result<Vec<Chip>, WebError> {
    let mut statement = c.prepare("SELECT id, symbol, name, color, category, image_url FROM assets WHERE id = ?1")?;
    let mut chips = vec![];
    for id in ids {
        let found = statement.query_row([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?,
                                                       r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?))).optional()?;
        let Some((id, symbol, name, color, category, image)) = found else { continue };
        let symbol = symbol.unwrap_or_default();
        chips.push(Chip {
            id, name: name.unwrap_or_default(), class: colors::ticker_class(category.as_deref(), color.as_deref()),
            pill: colors::pill_color(color.as_deref()).ok_or_else(|| WebError::Config("an asset colour 2.x cannot read".into()))?,
            logo: crate::web::tracker::row::asset_logo(image, color, symbol.clone(), category)?,
            symbol,
        });
    }
    Ok(chips)
}

/// `Exchange.find_by(id:)`'s name, as the sentence's exchange chip shows it.
pub fn exchange_name(c: &Connection, id: Option<i64>) -> Result<Option<String>, WebError> {
    let Some(id) = id else { return Ok(None) };
    Ok(c.query_row("SELECT name FROM exchanges WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0)).optional()?.map(Option::unwrap_or_default))
}

#[derive(Template)]
#[template(path = "wizard/_sheet_frame.html")]
struct SheetFrame<'a> { track: &'a str, content: &'a str }

#[derive(Template)]
#[template(path = "wizard/sheet.html")]
struct Sheet<'a> { close_url: String, close_label: String, track: &'a str, content: &'a str }

/// D-WIZARD-SHEET: a DCA step as the sheet Settings uses, the track beside the pane holding `content`. The frame around both is
/// `modal_content`, so every form, stream and transition that targets it is unchanged.
pub fn sheet(ctx: &Ctx, track: &str, content: &str) -> Result<String, WebError> { sheet_to(ctx, "/bots", track, content) }

/// The Assets step's sheet, whichever tab the switch shows (the Asset view, the Index view): its caption first in the pane,
/// drawn here once so the switch leaves it where it is.
pub fn assets_sheet(ctx: &Ctx, track: &str, content: &str) -> Result<String, WebError> {
    sheet(ctx, track, &format!("<h3 class=\"label\">{}</h3>\n{content}", crate::web::i18n::escape(&ctx.t("bot.setup.pick_assets"))))
}

/// D-SHEET-EVERYWHERE: `sheet` closing to `close` (the tracker's, the rules' and the bots' page). An empty `track` leaves the
/// side column empty, which the sheet does not draw.
pub fn sheet_to(ctx: &Ctx, close: &str, track: &str, content: &str) -> Result<String, WebError> {
    Ok(Sheet { close_url: ctx.path(close), close_label: ctx.t("button.close"), track, content }.render()?)
}

/// One item of a track that is not the DCA sentence's (D-SHEET-EVERYWHERE): its `bot.setup.progress_steps` label, or the chosen
/// venue's name, or the chosen tickers; and, when given, the link back to the step that chose it.
pub struct Stop { pub key: &'static str, pub name: Option<String>, pub chips: Vec<Chip>, pub back: Option<String> }

impl Stop {
    pub fn new(key: &'static str) -> Self { Stop { key, name: None, chips: vec![], back: None } }
    pub fn name(mut self, name: Option<String>) -> Self { self.name = name; self }
    pub fn chips(mut self, chips: Vec<Chip>) -> Self { self.chips = chips; self }
    pub fn back(mut self, url: String) -> Self { self.back = Some(url); self }
}

/// The track of `stops`, the one at `current` current, those before it done.
pub fn track(ctx: &Ctx, current: usize, stops: Vec<Stop>) -> Result<String, WebError> {
    let items = stops.into_iter().enumerate().map(|(place, stop)| {
        let state = if place < current { "done" } else if place == current { "current" } else { "upcoming" };
        let more = stop.chips.len().saturating_sub(5);
        let stack_class = match stop.chips.len() { 0 | 1 => "", 2 => "ticker-group--2", _ => "ticker-group--many" };
        Item { state, label: stop.name.unwrap_or_else(|| ctx.t(&format!("bot.setup.progress_steps.{}", stop.key))), more, stack_class,
               chips: stop.chips.into_iter().take(5).collect(), action: stop.back.map_or_else(Action::plain, |url| Action::get(url, true)) }
    }).collect();
    Ok(Track { items, csrf: "" }.render()?)
}

/// What `sheet` puts in its `modal_content` frame, for a stream that updates the frame.
pub fn sheet_frame(track: &str, content: &str) -> Result<String, WebError> {
    Ok(SheetFrame { track, content }.render()?)
}

/// What a DCA step knows of the wizard: the slots of 2.x's bots/wizard/_sentence ("Buy {assets} on {exchange} spending
/// {quote}"), which this build draws as the track and the pane's search box (D-WIZARD-NO-SENTENCE).
pub struct SentenceOf<'a> {
    pub order: &'a StepOrder,
    pub step: Step,
    /// Whether the bot is a basket (the multi namespace, or anything chosen on the asset step).
    pub multi: bool,
    pub chips: Vec<Chip>,
    pub exchange: Option<String>,
}

/// What a track item does when clicked: the request the sentence's slot made in the same state, or nothing.
pub struct Action { kind: &'static str, url: String, fields: Vec<(&'static str, &'static str)>, back: bool }

impl Action {
    fn plain() -> Self { Action { kind: "plain", url: String::new(), fields: vec![], back: false } }
    fn get(url: String, back: bool) -> Self { Action { kind: "get", url, fields: vec![], back } }
    fn post(url: String, fields: Vec<(&'static str, &'static str)>, back: bool) -> Self { Action { kind: "post", url, fields, back } }
}

/// One item of the track (D-WIZARD-TRACK r4): Exchange, Assets or Currency, by position done, current or upcoming; its label,
/// or the venue's name, or the chosen tickers (five at most, `more` beyond); and what it does, by the sentence's slot state.
pub struct Item { state: &'static str, label: String, chips: Vec<Chip>, more: usize, stack_class: &'static str, action: Action }

#[derive(Template)]
#[template(path = "wizard/_track.html")]
struct Track<'a> { items: Vec<Item>, csrf: &'a str }

#[derive(Template)]
#[template(path = "wizard/_search.html")]
struct Search<'a> { frame: &'a str, url: &'a str, placeholder: &'a str }

/// A DCA step's track, and the search box its pane opens with (empty when the step has none).
pub struct Steer { pub track: String, pub search: String }

/// D-WIZARD-NO-SENTENCE, D-WIZARD-TRACK: the sentence's slot states, computed as bots/wizard/_sentence computes them, drawn as
/// the track's three items in the order in force and the pane's search box. Each item does what its slot did (a done step's
/// way back, the order switch, the asset step's advance), whatever its position; `actions: false` draws them all plain (the
/// stock-broker step, which has no sentence).
pub fn steer(ctx: &Ctx, csrf: &str, of: SentenceOf, actions: bool) -> Result<Steer, WebError> {
    let on_first_step = of.order.first() == of.step;
    let on_asset_step = of.step == Step::Currencies;
    let at = |step: Step| of.order.steps.iter().position(|s| *s == step);
    let exchange_before_assets = at(Step::Exchange) < at(Step::Currencies).or_else(|| at(Step::Assets));
    let filled = of.exchange.is_some();
    let invite_only = of.step == Step::Exchange && on_first_step && of.chips.is_empty();
    let exchange_url = ctx.path(if of.multi { "/bots/dca_multi_assets/pick_exchange/new" } else { "/bots/dca_single_assets/pick_exchange/new" });
    let (advance_url, order_url) = (ctx.path("/bots/dca_single_assets/pick_buyable_asset/advance"), ctx.path("/bots/dca_single_assets/order"));
    let quote_search_url = ctx.path("/bots/dca_multi_assets/pick_spendable_asset/new");
    let exchange = if of.step == Step::Exchange { "active" }
        else if filled && on_asset_step { "filled_advance" }
        else if filled { "filled" }
        else if on_first_step && of.chips.is_empty() { "switch" }
        else if on_asset_step { "advance" }
        else { "inert" };
    let quote = if of.chips.is_empty() || !filled { "absent" } else if of.step == Step::Spendable { "search" } else if on_asset_step { "advance" } else { "inert" };
    let exchange_action = match (invite_only, exchange) {
        (true, _) | (_, "active" | "inert") => Action::plain(),
        (_, "filled") => Action::get(exchange_url.clone(), true),
        (_, "filled_advance") => Action::post(advance_url.clone(), vec![("to", "exchange")], exchange_before_assets),
        (_, "switch") => Action::post(order_url.clone(), vec![("flow", "exchange_first")], false),
        _ => Action::post(advance_url.clone(), vec![], false),
    };
    let assets_action = if on_asset_step { Action::plain() }
        else if !of.chips.is_empty() { Action::get(ctx.path("/bots/dca_single_assets/pick_buyable_asset/new"), true) }
        else { Action::post(order_url.clone(), vec![("flow", "asset_first")], false) };
    let quote_action = if !invite_only && quote == "advance" { Action::post(advance_url.clone(), vec![], false) } else { Action::plain() };
    let search = match (invite_only, exchange, quote) {
        (false, "active", _) => Search { frame: "exchange-picker", url: &exchange_url, placeholder: "exchange" }.render()?,
        (_, _, "search") => Search { frame: "asset-picker", url: &quote_search_url, placeholder: "USD" }.render()?,
        _ => String::new(),
    };
    // Position: the key and broker steps are the exchange's item.
    let item_of = |step: Step| match step { Step::Exchange | Step::Api => 0, Step::Currencies | Step::Assets => 1, Step::Spendable => 2 };
    let mut order: Vec<usize> = vec![];
    for step in of.order.steps { let item = item_of(*step); if !order.contains(&item) { order.push(item); } }
    let current = order.iter().position(|item| *item == item_of(of.step));
    let more = of.chips.len().saturating_sub(5);
    let stack_class = match of.chips.len() { 0 | 1 => "", 2 => "ticker-group--2", _ => "ticker-group--many" };
    let mut chips = Some(of.chips.into_iter().take(5).collect::<Vec<_>>());
    let mut actions_by_item = [Some(exchange_action), Some(assets_action), Some(quote_action)];
    let mut items = vec![];
    for (place, item) in order.iter().enumerate() {
        let state = match current { Some(current) if place < current => "done", Some(current) if place == current => "current", _ => "upcoming" };
        let action = actions_by_item[*item].take().filter(|_| actions).unwrap_or_else(Action::plain);
        let key = ["exchange", "assets", "spendable"][*item];
        let mut entry = Item { state, label: ctx.t(&format!("bot.setup.progress_steps.{key}")), chips: vec![], more: 0, stack_class: "", action };
        match item {
            0 => if let (Some(name), false) = (&of.exchange, of.step == Step::Exchange) { entry.label = name.clone(); },
            1 => { entry.chips = chips.take().unwrap_or_default(); entry.more = more; entry.stack_class = stack_class; }
            _ => {}
        }
        items.push(entry);
    }
    Ok(Steer { track: Track { items, csrf }.render()?, search })
}

/// Bots::WizardSessionGuard#redirect_if_session_expired: a guarded POST with no wizard begun (`session[:bot_config].blank?`) is
/// sent to the root by a turbo-stream redirect.
pub fn expired(ctx: &Ctx) -> Option<Response> {
    let blank = !present(ctx.session.lock().bot_config.as_ref());
    blank.then(|| stream(StatusCode::OK, super::turbo::redirect(&ctx.path("/"))))
}

/// 3.0 A26 (D-LEGACY-STALE-CREATE): the old create forms (`POST /bots/dca_multi_assets`, `POST /bots/dca_indexes`,
/// `/bots/dca_indexes/confirm_settings(/new)`), which Rust's wizards replaced with a create at their last step. Nothing
/// is read from the form or the session: no bot is made, and the wizard's state is dropped. The browser goes to the
/// wizard's start, as a Turbo Stream when it asked for one (how 2.x's create left the modal). A 2.x tab never gets
/// here: it carries 2.x's cookie and token, which the pipeline turns back first.
pub async fn stale(Extension(ctx): Extension<Ctx>) -> Response {
    ctx.session.lock().bot_config = None;
    let start = ctx.path("/bots/new");
    if ctx.stream { return stream(StatusCode::OK, super::turbo::redirect(&start)) }
    layout::redirect(if ctx.method == axum::http::Method::GET { StatusCode::FOUND } else { StatusCode::SEE_OTHER }, &start)
}

pub fn stream(status: StatusCode, body: String) -> Response {
    use axum::response::IntoResponse;
    (status, [(axum::http::header::CONTENT_TYPE, super::turbo::CONTENT_TYPE)], body).into_response()
}

#[derive(Template)]
#[template(path = "bots/new.html")]
struct New<'a> {
    v: &'a Ctx,
    advanced: bool,
}

/// GET /bots/new: every new bot config starts fresh (`session[:bot_config] = {}`), the one GET of the wizard
/// that writes. The chooser (Portfolio and Signal) shows only when the user enabled advanced bots; otherwise the portfolio
/// wizard's first step.
pub async fn new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    ctx.session.lock().bot_config = Some(json!({}));
    let (inner, owner) = (app.clone(), user.clone());
    let (advanced, shell) = app.db(move |c| {
        let advanced = c.query_row("SELECT advanced_bots_enabled FROM users WHERE id = ?1", [owner.id], |r| r.get::<_, bool>(0))?;
        Ok((advanced, if advanced { Some(Shell::load(c, &inner, &owner)?) } else { None }))
    }).await?;
    // D-CHOOSER-SKIPPED: with nothing but the portfolio wizard to choose (the index is picked on its Assets step), its first step.
    let Some(shell) = shell else { return Ok(layout::redirect(StatusCode::FOUND, &ctx.path("/bots/dca_single_assets/pick_exchange/new"))) };
    let csrf = ctx.csrf_token();
    // D-SHEET-NO-DIALOG: the chooser in the sheet, with no track.
    let body = sheet(&ctx, "", &New { v: &ctx, advanced }.render()?)?;
    shell::application(&ctx, &csrf, &user, &shell, Page { status: StatusCode::OK, body, flash_now: vec![] })
}

/// POST /bots/dca_single_assets/order: choose asset-first or exchange-first, wipe what the new order's first
/// step and those after it own, and go to that step. POST only: Turbo prefetches GETs on hover.
pub async fn order(Extension(ctx): Extension<Ctx>) -> Response {
    if ctx.user().is_none() { return auth::unauthenticated(&ctx) }
    let (flow, variant) = match ctx.params.form("flow").or_else(|| ctx.params.query("flow")) {
        Some("exchange_first") => ("exchange_first", Variant::ExchangeFirst),
        _ => ("asset_first", Variant::AssetFirst),
    };
    let order = StepOrder::new(BotType::Single, variant);
    {
        let mut session = ctx.session.lock();
        let config = session.bot_config.get_or_insert_with(|| json!({}));
        if !config.is_object() { *config = json!({}); }
        config["flow"] = json!(flow);
        reset_downstream(config, &order, order.first());
    }
    let first = match order.first() {
        Step::Exchange => "/bots/dca_single_assets/pick_exchange/new",
        _ => "/bots/dca_single_assets/pick_buyable_asset/new",
    };
    layout::redirect(StatusCode::FOUND, &ctx.path(first))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_order_matches_v2() {
        use Step::*;
        let single_asset_first = StepOrder::new(BotType::Single, Variant::AssetFirst);
        assert_eq!(single_asset_first.first(), Currencies);
        assert_eq!(single_asset_first.next_after(Exchange), Some(Api));
        assert_eq!(single_asset_first.next_after(Spendable), None);
        assert_eq!(single_asset_first.next_after(Assets), None, "not a step of this order");
        // Re-picking the first step is a full wipe; an asset step clears the assets.
        assert_eq!(single_asset_first.reset_keys(Currencies), ALL_WIZARD_KEYS.to_vec());
        assert_eq!(single_asset_first.reset_keys(Exchange), vec![EXCHANGE_KEY, BASE0_KEY, BASE1_KEY, QUOTE_KEY]);
        let exchange_first = StepOrder::new(BotType::Single, Variant::ExchangeFirst);
        // The exchange step keeps the sticky assets, even as the first step.
        assert_eq!(exchange_first.reset_keys(Exchange), vec![EXCHANGE_KEY, BASE0_KEY, BASE1_KEY, QUOTE_KEY]);
        assert_eq!(exchange_first.reset_keys(Api), vec![BASE0_KEY, BASE1_KEY, QUOTE_KEY]);
        assert_eq!(exchange_first.reset_keys(Currencies), vec![BASE_KEY, BASE0_KEY, BASE1_KEY, BASE_IDS_KEY, QUOTE_KEY]);
        assert_eq!(exchange_first.reset_keys(Spendable), vec![BASE0_KEY, BASE1_KEY, QUOTE_KEY]);
        assert_eq!(StepOrder::new(BotType::Multi, Variant::AssetFirst).reset_keys(Assets), ALL_WIZARD_KEYS.to_vec());
    }

    #[test]
    fn chosen_ids_read_either_shape() {
        assert_eq!(chosen_ids(Some(&json!({"settings": {"base_asset_ids": ["3", 3, 0, null, 5]}}))), vec![3, 5]);
        assert_eq!(chosen_ids(Some(&json!({"settings": {"base_asset_ids": [], "base_asset_id": "7"}}))), vec![7], "an empty list falls back to the one asset");
        assert_eq!(exchange_id(Some(&json!({"exchange_id": " "}))), None);
        assert_eq!(exchange_id(Some(&json!({"exchange_id": "4"}))), Some(4));
    }

    #[test]
    fn reset_downstream_removes_leaves_and_keeps_emptied_parents() {
        let mut config = json!({"flow": "asset_first", "exchange_id": 3, "settings": {"base_asset_ids": [1], "quote_asset_id": 2}});
        let order = StepOrder::new(BotType::Single, Variant::AssetFirst);
        reset_downstream(&mut config, &order, order.first());
        assert_eq!(config, json!({"flow": "asset_first", "settings": {}}));
        let mut config = json!({"flow": "exchange_first", "exchange_id": 3, "settings": {"base_asset_ids": [1], "quote_asset_id": 2}});
        let order = StepOrder::new(BotType::Single, Variant::ExchangeFirst);
        reset_downstream(&mut config, &order, order.first());
        assert_eq!(config, json!({"flow": "exchange_first", "settings": {"base_asset_ids": [1]}}));
    }

    #[test]
    fn completeness_reads_the_session_and_the_api_key_from_the_database() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE api_keys (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, key_type INTEGER, status INTEGER);
                         INSERT INTO api_keys VALUES (1, 7, 3, 0, 1), (2, 7, 4, 0, 2), (3, 7, 5, 1, 1);").unwrap();
        let order = StepOrder::new(BotType::Single, Variant::ExchangeFirst);
        let config = |exchange: Value| json!({"exchange_id": exchange, "settings": {"base_asset_ids": [null, 9]}});
        assert!(step_complete(&c, 7, Some(&config(json!(3))), Step::Api).unwrap());
        assert!(step_complete(&c, 7, Some(&config(json!("3"))), Step::Api).unwrap(), "a form's id is a string");
        assert!(!step_complete(&c, 7, Some(&config(json!(4))), Step::Api).unwrap(), "incorrect");
        assert!(!step_complete(&c, 7, Some(&config(json!(5))), Step::Api).unwrap(), "not a trading key");
        assert!(!step_complete(&c, 8, Some(&config(json!(3))), Step::Api).unwrap(), "another user's");
        assert!(step_complete(&c, 7, Some(&config(json!(3))), Step::Assets).unwrap());
        assert!(!step_complete(&c, 7, Some(&json!({"settings": {"base_asset_ids": [null]}})), Step::Assets).unwrap());
        assert!(!step_complete(&c, 7, Some(&json!({"exchange_id": " "})), Step::Exchange).unwrap(), "blank");
        assert_eq!(first_incomplete(&c, 7, None, &order).unwrap(), Step::Exchange);
        assert_eq!(prerequisite(&c, 7, Some(&config(json!(4))), &order, Step::Currencies).unwrap(), Some(Step::Api));
        assert_eq!(prerequisite(&c, 7, Some(&config(json!(3))), &order, Step::Spendable).unwrap(), None);
        assert_eq!(current_variant(None), Variant::ExchangeFirst);
        assert_eq!(current_variant(Some(&json!({"flow": "asset_first"}))), Variant::AssetFirst);
    }
}
