//! The dashboard's Merge (Bots::MergesController, Bot::Merge): several baskets and index bots become one stopped basket that
//! owns every order, activity row and membership they had, and the sources are soft-deleted. `new` is the confirmation modal
//! for the bots the user picked (the first is the anchor), `create` merges exactly the bots the modal named, in one write
//! transaction that calls no venue and no market-data service.
//!
//! The fence is per bot (write::engine_holds, D-FENCE-PER-BOT): Rails takes every source venue's lease and refuses while a job
//! holds one; this build refuses while the engine holds a source and bumps every source's run generation.
//! D-MERGE-REVERSAL-PENDING: a source whose ⇄ reversal still has cancels or reserve credits to settle is refused (Rails
//! merges it), so the merged bot is never credited a carry Rails does not move.
use super::{index_switch, write, Bot, For, Kind};
use crate::engine::{eligibility, model, reversal};
use crate::enums::BotStatus;
use crate::figures::{at::At, db::Subject, dec::Dec, walk::{self, Metrics}, FiguresError};
use crate::web::{bots, colors, i18n, layout::Ctx, wizard, WebError};
use askama::Template;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};

/// Bot::Merge::TYPES.
pub(super) const TYPES: [&str; 2] = ["Bots::DcaMultiAsset", "Bots::DcaIndex"];

/// The ids as `Array(params[:ids]).map(&:to_i).uniq` reads them (request order: the first is the anchor), and the venue
/// picked in the modal (`params[:exchange_id].presence&.to_i`).
pub fn params(value: &Value) -> (Vec<i64>, Option<i64>) {
    let mut ids = vec![];
    for id in self::ids(value, "ids") { if !ids.contains(&id) { ids.push(id); } }
    let exchange = match value.get("exchange_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if crate::ruby::blank(text) => None,
        Some(other) => Some(wizard::to_i(other)),
    };
    (ids, exchange)
}

/// `Array(params[key]).map(&:to_i)`, in request order, repeats kept.
pub(super) fn ids(value: &Value, key: &str) -> Vec<i64> {
    match value.get(key) {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(items)) => items.iter().map(wizard::to_i).collect(),
        Some(other) => vec![wizard::to_i(other)],
    }
}

struct Source {
    bot: Bot,
    engine: model::Bot,
    class: String,
    position: i64,
    /// `redeploy_declined_offset`, as stored.
    offset: String,
}

/// Why the merge cannot go ahead: a key under errors.bots.merge, with what it names.
/// `Unrun`: this build's own refusal (engine.write_refused), a venue it does not run the merged bot on (eligibility::admission).
enum Reason { Plain(&'static str), Bot(&'static str, String), Open(String, String), Unrun(&'static str) }

pub struct Merge {
    ids: Vec<i64>,
    exchange_id: Option<i64>,
    bots: Vec<Source>,
    /// Every source's current target weights summed per asset, anchor first, over the members that trade somewhere at the quote.
    weights: Vec<(i64, f64)>,
    /// `{ asset => venues }` of every summed member (Bot::Merge.venues_for).
    venues: Vec<(i64, Vec<i64>)>,
    /// The merged bot's venue: (id, name).
    exchange: Option<(i64, String)>,
    /// The sources' fresh books, read by `reason` (in source order).
    books: Vec<Metrics>,
}

/// What loading the selection found.
pub enum Loaded { Merge(Box<Merge>), Unported(&'static str) }

pub(super) fn fig(e: FiguresError) -> WebError { super::data(format!("merge books: {e:?}")) }

fn exchange_name(c: &Connection, id: i64) -> Result<Option<String>, WebError> {
    Ok(c.query_row("SELECT name FROM exchanges WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0)).optional()?.map(Option::unwrap_or_default))
}

/// Why this build would not run the merged bot (a basket that buys) on this venue, if it would not.
pub(super) fn unrun(c: &Connection, exchange: i64) -> Result<Option<&'static str>, WebError> {
    let class: Option<String> = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [exchange], |r| r.get(0)).optional()?.flatten();
    Ok(eligibility::admission(class.as_deref(), false, false))
}

fn waiting(c: &Connection, bot: i64) -> Result<bool, WebError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id = ?1 AND status = 0 AND external_status IN (0, 1))", [bot], |r| r.get(0))?)
}

/// D-MERGE-REVERSAL-PENDING: cancels a reversal captured or reserves it registered, not settled yet.
pub(super) fn reversal_pending(bot: &model::Bot) -> bool {
    [reversal::RESERVE_PENDING, reversal::CANCELS].iter().any(|key| bot.transient.get(*key).and_then(Value::as_array).is_some_and(|a| !a.is_empty()))
}

impl Merge {
    /// `Bot::Merge.new(current_user, ids, exchange_id:)`: the user's bots that are not deleted, in request order.
    pub fn load(c: &Connection, ctx: &Ctx, owner: i64, ids: Vec<i64>, exchange_id: Option<i64>) -> Result<Loaded, WebError> {
        let wash: Option<bool> = c.query_row("SELECT wash_sale_enabled FROM users WHERE id = ?1", [owner], |r| r.get(0))?;
        let (provider, _) = bots::market_data(c, &ctx.app)?;
        let mut sources = vec![];
        for id in &ids {
            let row: Option<(Option<String>, i64, String)> = c.query_row(
                "SELECT type, position, CAST(redeploy_declined_offset AS TEXT) FROM bots WHERE id = ?1 AND user_id = ?2 AND status <> 3",
                (id, owner), |r| Ok((r.get(0)?, r.get::<_, Option<i64>>(1)?.unwrap_or(0), r.get::<_, Option<String>>(2)?.unwrap_or_else(|| "0".into())))).optional()?;
            let Some((class, position, offset)) = row else { continue };
            if let Some(reason) = super::refusal(c, *id, wash, provider, For::Settings)? { return Ok(Loaded::Unported(reason)); }
            let Some(bot) = Bot::find(c, owner, *id, For::Settings, ctx.locale)? else { continue };
            if let Some(reason) = bot.unrendered() { return Ok(Loaded::Unported(reason)); }
            sources.push(Source { engine: model::load_bot(c, *id)?, bot, class: class.unwrap_or_default(), position, offset });
        }
        let mut merge = Merge { ids, exchange_id, bots: sources, weights: vec![], venues: vec![], exchange: None, books: vec![] };
        if merge.bots.is_empty() { return Ok(Loaded::Merge(Box::new(merge))); }
        // `weights`: each source's current_allocations (in-index rows, heaviest first), `to_f`, summed per asset.
        let mut summed: Vec<(i64, f64)> = vec![];
        for source in &merge.bots {
            let mut s = c.prepare_cached("SELECT asset_id, CAST(target_allocation AS REAL) FROM bot_index_assets WHERE bot_id = ?1 AND in_index = 1 ORDER BY target_allocation DESC")?;
            for row in s.query_map([source.bot.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<f64>>(1)?.unwrap_or(0.0))))? {
                let (asset, weight) = row?;
                match summed.iter_mut().find(|(id, _)| *id == asset) { Some(entry) => entry.1 += weight, None => summed.push((asset, weight)) }
            }
        }
        let quote = merge.anchor().bot.setting("quote_asset_id").and_then(Value::as_i64);
        for (asset, _) in &summed {
            let listed = bots::venues_for(c, *asset, quote)?;
            if !listed.is_empty() { merge.venues.push((*asset, listed)); }
        }
        merge.weights = summed.into_iter().filter(|(asset, _)| merge.venues.iter().any(|(id, _)| id == asset)).collect();
        let common = merge.common_exchange_ids();
        // Unpicked, the default is one this build runs the merged bot on, if any is common (`reason` refuses any other).
        let mut runnable = vec![];
        for id in &common { if unrun(c, *id)?.is_none() { runnable.push(*id); } }
        let offered = if runnable.is_empty() { &common } else { &runnable };
        let anchor_venue = merge.anchor().bot.exchange.id;
        let chosen = match merge.exchange_id {
            Some(pick) => common.contains(&pick).then_some(pick),
            None if offered.contains(&anchor_venue) => Some(anchor_venue),
            None => bots::connected_exchange_ids(c, owner)?.into_iter().find(|id| offered.contains(id)).or_else(|| offered.iter().min().copied()),
        };
        merge.exchange = match chosen { Some(id) => exchange_name(c, id)?.map(|name| (id, name)), None => None };
        Ok(Loaded::Merge(Box::new(merge)))
    }

    fn anchor(&self) -> &Source { &self.bots[0] }

    /// Exchanges that list every tradeable member at the quote.
    fn common_exchange_ids(&self) -> Vec<i64> {
        let mut sets = self.weights.iter().map(|(asset, _)| self.venues.iter().find(|(id, _)| id == asset).map(|(_, v)| v.clone()).unwrap_or_default());
        let Some(first) = sets.next() else { return vec![] };
        let mut common: Vec<i64> = sets.fold(first, |acc, set| acc.into_iter().filter(|id| set.contains(id)).collect());
        common.sort_unstable();
        common
    }

    /// Bot::Merge.mergeable?, and D-MERGE-REVERSAL-PENDING.
    fn mergeable(c: &Connection, source: &Source) -> Result<bool, WebError> {
        let members = source.bot.memberships.iter().any(|m| m.in_index);
        Ok(TYPES.contains(&source.class.as_str()) && source.bot.kind != Kind::Signal
            && !matches!(source.bot.status, BotStatus::Archived | BotStatus::Deleted | BotStatus::Executing)
            && !index_switch::in_flight(c, &source.engine)? && members && !reversal_pending(&source.engine))
    }

    /// Bot::Merge#reason, in its order. The books are read fresh here, as Rails' `reason` reads them for verify_books!.
    fn reason(&mut self, c: &Connection, now: At) -> Result<Option<Reason>, WebError> {
        if self.bots.len() < self.ids.len() { return Ok(Some(Reason::Plain("missing"))); }
        if self.bots.len() < 2 { return Ok(Some(Reason::Plain("too_few"))); }
        for source in &self.bots {
            if !Self::mergeable(c, source)? { return Ok(Some(Reason::Bot("unavailable", source.bot.label.clone()))); }
        }
        let quote = |source: &Source| source.bot.setting("quote_asset_id").map_or(0, wizard::to_i);
        if !self.bots.iter().all(|source| quote(source) == quote(self.anchor())) { return Ok(Some(Reason::Plain("quote"))); }
        if self.weights.is_empty() { return Ok(Some(Reason::Plain("nothing_to_buy"))); }
        let Some((exchange, _)) = self.exchange.clone() else { return Ok(Some(Reason::Plain("no_common_exchange"))) };
        if let Some(why) = unrun(c, exchange)? { return Ok(Some(Reason::Unrun(why))); }
        // A resting order is polled through its bot's exchange; on another venue it would never be found.
        for source in &self.bots {
            if source.bot.exchange.id != exchange && waiting(c, source.bot.id)? {
                return Ok(Some(Reason::Open(source.bot.label.clone(), source.bot.exchange.name.clone())));
            }
        }
        self.books = vec![];
        for source in &self.bots {
            let books = walk::metrics(c, &Subject::load(c, source.bot.id).map_err(fig)?, now).map_err(fig)?;
            if books.walked.as_ref().is_some_and(|w| w.external_sales) { return Ok(Some(Reason::Bot("external_sales", source.bot.label.clone()))); }
            self.books.push(books);
        }
        Ok(None)
    }

    fn translate(locale: &str, reason: &Reason) -> String {
        let key = |k: &str| format!("errors.bots.merge.{k}");
        match reason {
            Reason::Plain(k) => i18n::text(locale, &key(k), &[]),
            Reason::Bot(k, label) => i18n::text(locale, &key(k), &[("label", i18n::Arg::Text(label))]),
            Reason::Open(label, exchange) => i18n::text(locale, &key("open_orders"), &[("label", i18n::Arg::Text(label)), ("exchange", i18n::Arg::Text(exchange))]),
            Reason::Unrun(why) => write_refused(locale, why),
        }
    }

    /// The anchor's `unavailable`: a source the engine holds right now (the lease Rails could not take).
    fn unavailable(&self, locale: &str) -> String { Self::translate(locale, &Reason::Bot("unavailable", self.anchor().bot.label.clone())) }

    /// `{asset id => weight}`: the summed weights, normalised by the basket's own normaliser (3 dp, sums to 1).
    fn allocations(&self) -> Result<Map<String, Value>, WebError> {
        let mut weights: Map<String, Value> = self.weights.iter().map(|(asset, weight)| (asset.to_string(), json!(weight))).collect();
        super::draft::normalize(&mut weights)?;
        Ok(weights)
    }

    /// The venues the user may pick from: every common one this build runs the merged bot on, or only the one the sources'
    /// resting orders are polled through (none when they rest on two), by name.
    fn exchanges(&self, c: &Connection) -> Result<Vec<(i64, String)>, WebError> {
        let mut resting: Vec<i64> = vec![];
        for source in &self.bots {
            if waiting(c, source.bot.id)? && !resting.contains(&source.bot.exchange.id) { resting.push(source.bot.exchange.id); }
        }
        let common = self.common_exchange_ids();
        let mut ids: Vec<i64> = match resting.as_slice() { [] => common, [one] => common.into_iter().filter(|id| id == one).collect(), _ => vec![] };
        let mut runnable = vec![];
        for id in ids.drain(..) { if unrun(c, id)?.is_none() { runnable.push(id); } }
        let ids = runnable;
        let mut s = c.prepare("SELECT id, name FROM exchanges WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY name")?;
        let rows = s.query_map([json!(ids).to_string()], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// One stacked asset of the modal.
pub(super) struct Pill { pub(super) symbol: String, pub(super) class: &'static str, pub(super) color: String, pub(super) id: i64 }

/// The modal's stacked assets, in this order (an id with no asset row is left out).
pub(super) fn pills(c: &Connection, ids: &[i64]) -> Result<Vec<Pill>, WebError> {
    let mut pills = vec![];
    for id in ids {
        let Some(asset) = super::Asset::find(c, *id)? else { continue };
        pills.push(Pill {
            symbol: asset.symbol().to_string(), class: colors::ticker_class(asset.category.as_deref(), asset.color.as_deref()),
            color: colors::pill_color(asset.color.as_deref()).ok_or_else(|| WebError::Config(format!("asset {id}: its colour is not a colour")))?, id: *id,
        });
    }
    Ok(pills)
}

/// The modal's `ticker-group--…` class for this many stacked assets.
pub(super) fn stack_class(pills: usize) -> &'static str { match pills { 0 | 1 => "", 2 => "ticker-group--2", _ => "ticker-group--many" } }
struct VenueOption { id: i64, name: String, selected: bool }

#[derive(Template)]
#[template(path = "bots/merges/new.html")]
struct Modal<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    title: String,
    refusal: Option<String>,
    ids: Vec<i64>,
    explanation: String,
    venues: Vec<VenueOption>,
    exchange_id: i64,
    stack_class: &'static str,
    pills: Vec<Pill>,
    assets_stay: bool,
}

/// bots/merges/new: the modal, refusal included, for the selection as it stands.
pub fn modal(c: &Connection, ctx: &Ctx, merge: &mut Merge, csrf: &str) -> Result<String, WebError> {
    let now = At::from_utc(ctx.now).ok_or_else(|| super::data("the merge's time is out of range".into()))?;
    let refusal = merge.reason(c, now)?.map(|reason| Merge::translate(ctx.locale, &reason));
    let title = if refusal.is_some() { ctx.t("button.merge") } else { i18n::text(ctx.locale, "bot.merge.title", &[("count", i18n::Arg::Count(merge.bots.len() as i64))]) };
    let mut view = Modal { v: ctx, csrf, title, ids: merge.bots.iter().map(|s| s.bot.id).collect(), explanation: String::new(), venues: vec![], exchange_id: 0, stack_class: "", pills: vec![], assets_stay: false, refusal };
    if view.refusal.is_none() {
        let (exchange, name) = merge.exchange.clone().ok_or_else(|| super::data("a merge without a venue".into()))?;
        let venues = merge.exchanges(c)?;
        let shown = if venues.len() > 1 {
            view.venues = venues.into_iter().map(|(id, name)| VenueOption { selected: id == exchange, id, name }).collect();
            format!("\n                <span class=\"sinput sinput--small sinput--select merge-modal__exchange\">\n                  {}\n                  {}\n                </span>\n",
                    i18n::escape(&name), select(&view.venues))
        } else { i18n::escape(&name) };
        let quote = merge.anchor().bot.quote_symbol().unwrap_or("").to_string();
        view.explanation = i18n::t(ctx.locale, "bot.merge.explanation_html", &[("quote", i18n::Arg::Text(&quote)), ("exchange", i18n::Arg::Html(&shown))]);
        let ids: Vec<i64> = merge.allocations()?.keys().map(|key| crate::web::bot::search::to_i(key)).collect();
        view.pills = pills(c, &ids)?;
        view.stack_class = stack_class(view.pills.len());
        view.assets_stay = merge.bots.iter().any(|source| source.bot.exchange.id != exchange);
        view.exchange_id = exchange;
    }
    Ok(view.render()?)
}

/// `select_tag :exchange_id, options_from_collection_for_select(…)`.
fn select(venues: &[VenueOption]) -> String {
    let options: Vec<String> = venues.iter().map(|o| format!("<option {}value=\"{}\">{}</option>", if o.selected { "selected=\"selected\" " } else { "" }, o.id, i18n::escape(&o.name))).collect();
    format!("<select name=\"exchange_id\" data-action=\"change-&gt;form--submit#submit\">{}</select>", options.join("\n"))
}

pub enum Merged { Done(i64), Refused(String), Unported(&'static str) }

/// POST /bots/merge (Bot::Merge#perform!), in one write transaction: the reason re-checked on the rows it reads, the fence,
/// the merged bot built and saved, every row moved, the memberships folded, the sources deleted, and the books verified;
/// a refusal of any of them rolls everything back.
pub fn perform(c: &Connection, ctx: &Ctx, owner: i64, ids: Vec<i64>, exchange_id: Option<i64>) -> Result<Merged, WebError> {
    match perform_inner(c, ctx, owner, ids, exchange_id) {
        Err(e) if super::unreadable(&e) => Ok(Merged::Unported("a bot beyond the supported history, member or numeric bounds")),
        other => other,
    }
}

fn perform_inner(c: &Connection, ctx: &Ctx, owner: i64, ids: Vec<i64>, exchange_id: Option<i64>) -> Result<Merged, WebError> {
    let now = ctx.app.now();
    let at = At::from_utc(now).ok_or_else(|| super::data("the merge's time is out of range".into()))?;
    let tx = model::immediate(c)?;
    let mut merge = match Merge::load(&tx, ctx, owner, ids, exchange_id)? { Loaded::Merge(m) => m, Loaded::Unported(reason) => return Ok(Merged::Unported(reason)) };
    if let Some(reason) = merge.reason(&tx, at)? { return Ok(Merged::Refused(Merge::translate(ctx.locale, &reason))); }
    for source in &merge.bots {
        if write::engine_holds(&tx, &source.engine)? { return Ok(Merged::Refused(merge.unavailable(ctx.locale))); }
    }
    let id = write_merged(&tx, ctx, owner, &merge, now)?;
    if !books_hold(&tx, &merge.books, id, at)? {
        tx.rollback()?;
        return Ok(Merged::Refused(Merge::translate(ctx.locale, &Reason::Plain("interleaved"))));
    }
    // The merged row as the redirect's page and the dashboard read it: a bot they would not serve is never committed.
    if let Some(reason) = unserved(&tx, ctx, owner, id)? {
        tx.rollback()?;
        return Ok(Merged::Refused(write_refused(ctx.locale, reason)));
    }
    if let Err(refusal) = eligibility::guard(&tx, &ctx.app.cipher, Some(id)) {
        tx.rollback()?;
        return Ok(Merged::Refused(write_refused(ctx.locale, &refusal.reason())));
    }
    tx.commit()?;
    Ok(Merged::Done(id))
}

pub(super) fn write_refused(locale: &str, reason: &str) -> String { i18n::text(locale, "engine.write_refused", &[("reason", i18n::Arg::Text(reason))]) }

/// Why this build would not serve this bot, as `load` asks of each source (For::Settings): the refusal, then the bot as it
/// renders. A page without figures (For::Page) refuses what the sources already showed it, so it is not asked here.
pub(super) fn unserved(c: &Connection, ctx: &Ctx, owner: i64, id: i64) -> Result<Option<&'static str>, WebError> {
    let wash: Option<bool> = c.query_row("SELECT wash_sale_enabled FROM users WHERE id = ?1", [owner], |r| r.get(0))?;
    let (provider, _) = bots::market_data(c, &ctx.app)?;
    if let Some(reason) = super::refusal(c, id, wash, provider, For::Settings)? { return Ok(Some(reason)); }
    Ok(Bot::find(c, owner, id, For::Settings, ctx.locale)?.and_then(|bot| bot.unrendered()))
}

/// Bot::Merge#bot and #write!.
fn write_merged(tx: &Connection, ctx: &Ctx, owner: i64, merge: &Merge, now: DateTime<Utc>) -> Result<i64, WebError> {
    let anchor = merge.anchor();
    let (exchange, _) = merge.exchange.clone().ok_or_else(|| super::data("a merge without a venue".into()))?;
    let setting = |key: &str| anchor.bot.settings.get(key).cloned().unwrap_or(Value::Null);
    // `user.bots.new(…)`: the schedule alone, the after_initialize defaults over it (no venue yet, so no ticker), then
    // `exchange=` with its limit-orders decorator, then the weights.
    let config = json!({ "settings": { "quote_asset_id": setting("quote_asset_id"), "quote_amount": setting("quote_amount"),
                                       "interval": setting("interval"), "weighting": "manual" } });
    let mut built = wizard::create::build(tx, &config, ctx.locale)?;
    // A default with no venue to read (nil) is no change to the store, so the key is never written.
    built.settings.retain(|_, value| !value.is_null());
    built.exchange_id = Some(exchange);
    let mut decorated = json!({ "exchange_id": exchange, "settings": Value::Object(std::mem::take(&mut built.settings)) });
    wizard::create::lock_limit_orders(tx, &mut decorated)?;
    built.settings = decorated["settings"].as_object().cloned().unwrap_or_default();
    let allocations = merge.allocations()?;
    let members: Vec<i64> = allocations.keys().map(|key| crate::web::bot::search::to_i(key)).collect();
    built.settings.insert("allocations".into(), Value::Object(allocations));
    built.set_members(tx, members, ctx.locale)?;
    let errors = wizard::create::validate(tx, &built, ctx.locale)?;
    // `save!`: a merged bot Rails cannot save raises.
    if !errors.is_empty() { return Err(super::data(format!("merged bot invalid: {}", errors.join(", ")))); }
    let id = wizard::create::insert_as(tx, owner, built, now, BotStatus::Stopped, anchor.position)?;

    let sources: Vec<i64> = merge.bots.iter().map(|s| s.bot.id).collect();
    let list = json!(sources).to_string();
    tx.execute("UPDATE transactions SET bot_id = ?1 WHERE bot_id IN (SELECT value FROM json_each(?2))", (id, &list))?;
    tx.execute("UPDATE bot_activity_logs SET bot_id = ?1 WHERE bot_id IN (SELECT value FROM json_each(?2))", (id, &list))?;
    fold_memberships(tx, id, &list, exchange, anchor.bot.setting("quote_asset_id").and_then(Value::as_i64), now)?;
    // `bots.sum(&:redeploy_declined_offset)`, a BigDecimal, written as its text (the column's affinity stores it as Rails' does).
    let mut offset = Dec::zero();
    for source in &merge.bots {
        let value = Dec::parse(&source.offset).map_err(|e| super::data(format!("bot {}: redeploy_declined_offset {e:?}", source.bot.id)))?;
        offset = (&offset + &value).map_err(|e| super::data(format!("redeploy_declined_offset {e:?}")))?;
    }
    tx.execute("UPDATE bots SET redeploy_declined_offset = ?1 WHERE id = ?2", (offset.to_s_f(), id))?;
    // merge_transient_data!: the merged history's last row and the resolved sales, then `.compact` drops every nil key.
    let mut resolved: Vec<i64> = vec![];
    for source in &merge.bots {
        let ids: Vec<i64> = match source.engine.transient.get("liquidation_resolved_orders") {
            None | Some(Value::Null) => vec![],
            Some(Value::Array(items)) => items.iter().map(wizard::to_i).collect(),
            Some(other) => vec![wizard::to_i(other)],
        };
        for id in ids { if !resolved.contains(&id) { resolved.push(id); } }
    }
    let last: Option<i64> = tx.query_row("SELECT MAX(id) FROM transactions WHERE bot_id = ?1", [id], |r| r.get(0))?;
    let text: String = tx.query_row("SELECT transient_data FROM bots WHERE id = ?1", [id], |r| r.get(0))?;
    let mut transient: Map<String, Value> = serde_json::from_str(&text).map_err(|e| super::data(format!("merged bot transient: {e}")))?;
    transient.insert("merged_history_until_id".into(), json!(last));
    transient.insert("liquidation_resolved_orders".into(), if resolved.is_empty() { Value::Null } else { json!(resolved) });
    transient.retain(|_, value| !value.is_null());
    tx.execute("UPDATE bots SET transient_data = ?1 WHERE id = ?2", (Value::Object(transient).to_string(), id))?;
    // The sources, deleted as the Delete button leaves a bot; their run generation moves, fencing an engine write in flight.
    tx.execute("UPDATE bots SET status = 3, stopped_at = ?1, \
                transient_data = json_set(transient_data, '$.run_generation', coalesce(json_extract(transient_data, '$.run_generation'), 0) + 1) \
                WHERE id IN (SELECT value FROM json_each(?2))", (crate::codec::format_time(now), &list))?;
    let labels: Vec<&str> = merge.bots.iter().map(|s| s.bot.label.as_str()).collect();
    model::log_activity(tx, id, "merged", model::Level::Info, json!({ "source_ids": sources, "source_labels": labels }), now)?;
    Ok(id)
}

/// Bot::Merge#fold_memberships!: one row per asset on the merged bot. A member's row (refresh_composition wrote it) keeps the
/// earliest entry; any other source row comes along as an exited row on the merged venue's ticker for the asset, else its own.
fn fold_memberships(tx: &Connection, id: i64, sources: &str, exchange: i64, quote: Option<i64>, now: DateTime<Utc>) -> Result<(), WebError> {
    // `.pluck(:base_asset_id, :id).to_h`: a later listing of the same asset wins.
    let mut local: Vec<(i64, i64)> = vec![];
    let mut s = tx.prepare("SELECT base_asset_id, id FROM tickers WHERE exchange_id = ?1 AND quote_asset_id IS ?2")?;
    for row in s.query_map((exchange, quote), |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?)))? {
        let (asset, ticker) = row?;
        let Some(asset) = asset else { continue };
        match local.iter_mut().find(|(a, _)| *a == asset) { Some(entry) => entry.1 = ticker, None => local.push((asset, ticker)) }
    }
    drop(s);
    let at = crate::codec::format_time(now);
    let mut rows = tx.prepare("SELECT id, asset_id, entered_at FROM bot_index_assets WHERE bot_id IN (SELECT value FROM json_each(?1)) ORDER BY id")?;
    let rows: Vec<(i64, i64, Option<String>)> = rows.query_map([sources], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    for (row, asset, entered) in rows {
        let existing: Option<(i64, Option<String>)> = tx.query_row("SELECT id, entered_at FROM bot_index_assets WHERE bot_id = ?1 AND asset_id = ?2 LIMIT 1", (id, asset),
                                                                   |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        match existing {
            Some((existing, current)) => {
                let earliest = [current.clone(), entered].into_iter().flatten().min_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
                if earliest.is_some() && earliest != current { tx.execute("UPDATE bot_index_assets SET entered_at = ?1 WHERE id = ?2", (earliest, existing))?; }
            }
            None => {
                let ticker = local.iter().find(|(a, _)| *a == asset).map(|(_, t)| *t);
                tx.execute("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, in_index, entered_at, exited_at, target_allocation, current_allocation, created_at, updated_at) \
                            SELECT ?1, asset_id, coalesce(?2, ticker_id), 0, entered_at, coalesce(exited_at, ?3), target_allocation, current_allocation, ?3, ?3 \
                            FROM bot_index_assets WHERE id = ?4", (id, ticker, &at, row))?;
            }
        }
    }
    Ok(())
}

/// Bot::Merge#verify_books!: the merged walk against the sum of the sources' books on the two figures no merge may change,
/// units per asset (by the asset behind each key) and net money in, each to 12 decimals.
fn books_hold(tx: &Connection, books: &[Metrics], id: i64, at: At) -> Result<bool, WebError> {
    let actual = walk::metrics(tx, &Subject::load(tx, id).map_err(fig)?, at).map_err(fig)?;
    crate::figures::budget::within(|| -> Result<bool, crate::figures::num::NumError> {
        let mut expected: Vec<(Identity, Dec)> = vec![];
        for data in books { for (identity, units) in units_by_identity(data)? { add(&mut expected, identity, &units)?; } }
        let actual_units = units_by_identity(&actual)?;
        let zero = Dec::zero();
        let get = |list: &[(Identity, Dec)], key: &Identity| list.iter().find(|(k, _)| k == key).map_or(zero.clone(), |(_, v)| v.clone());
        for key in expected.iter().map(|(k, _)| k).chain(actual_units.iter().map(|(k, _)| k)) {
            if get(&expected, key).round(12)? != get(&actual_units, key).round(12)? { return Ok(false); }
        }
        let mut money = Dec::zero();
        for data in books { money = (&money + &net_in(data)?)?; }
        Ok(money.round(12)? == net_in(&actual)?.round(12)?)
    }).map_err(|e| super::data(format!("merge books: {e:?}")))
}

/// A holding by the asset behind it, or, known only by its string, that string.
#[derive(Clone, PartialEq)]
enum Identity { Asset(i64), Text(String) }

fn add(list: &mut Vec<(Identity, Dec)>, identity: Identity, units: &Dec) -> Result<(), crate::figures::num::NumError> {
    match list.iter_mut().find(|(k, _)| *k == identity) { Some(entry) => entry.1 = (&entry.1 + units)?, None => list.push((identity, units.clone())) }
    Ok(())
}

fn units_by_identity(data: &Metrics) -> Result<Vec<(Identity, Dec)>, crate::figures::num::NumError> {
    let mut units = vec![];
    for (key, entry) in &data.asset_breakdown {
        let identity = match data.key_assets.iter().find(|(k, _)| k == key).and_then(|(_, asset)| *asset) {
            Some(asset) => Identity::Asset(asset),
            None => Identity::Text(data.key_strings.iter().find(|(k, _)| k == key).and_then(|(_, s)| s.first().cloned()).unwrap_or_else(|| key.clone())),
        };
        add(&mut units, identity, &entry.amount.to_d()?)?;
    }
    Ok(units)
}

/// Money in from outside less the money the books still hold, with every estimated sale's proceeds added back.
fn net_in(data: &Metrics) -> Result<Dec, crate::figures::num::NumError> {
    &(&data.total_quote_amount_invested.to_d()? - &data.rebalance_cash.to_d()?)? + &data.estimated_proceeds.to_d()?
}
