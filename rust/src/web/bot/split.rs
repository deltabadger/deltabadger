//! The dashboard's Split (Bots::SplitsController, Bot::Split): each picked basket or index bot becomes one stopped one-asset
//! basket per member that still trades on its venue at its quote, spending the source's amount times the member's weight
//! (re-normalised over the survivors) at its interval. Every order of an asset moves to that asset's bot as REGULAR, every
//! other row (an exited holding, an orphan) to the first (heaviest) child with the exited memberships, and the source is
//! soft-deleted. `new` is the modal, `create` (perform) splits exactly the bots it named, in one write transaction that calls
//! no venue and no market-data service; `complete` is what every caller runs once it committed.
//!
//! Divergences: D-FENCE-PER-BOT (merge.rs: a source the engine holds is `busy`, naming its venue; every source's run
//! generation moves). D-SPLIT-REVERSAL-PENDING: a source whose ⇄ reversal still has cancels or reserve credits to settle is
//! `unavailable` (Rails splits it and the credit stays on the deleted source). D-SPLIT-LEGACY-SHADOW: a row recorded without
//! its asset goes by figures::walk::listing_names over every asset of the source's history (Rails matches symbols exactly);
//! one that names assets bound for different children refuses the split instead of guessing. A redeploy in progress (a
//! waiting REDEPLOY row, or a Bot::RedeployJob Rails still holds) is not served: this build runs no redeploy. A job queue it
//! can't read is refused the same way, never read as "no job".
use super::{index_switch, merge, write, Bot, For, Kind};
use crate::engine::{eligibility, model};
use crate::enums::BotStatus;
use crate::figures::{at::At, db::{self as figures_db, Subject}, dec::Dec, num::Num, walk::{self, Metrics}};
use crate::web::{bots, figure, i18n, layout::Ctx, wizard, App, WebError};
use askama::Template;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

/// `Array(params[:ids]).map(&:to_i).uniq` (request order) and `Array(params[:keep_ids]).map(&:to_i)`.
pub fn params(value: &Value) -> (Vec<i64>, Vec<i64>) {
    let mut ids = vec![];
    for id in merge::ids(value, "ids") { if !ids.contains(&id) { ids.push(id); } }
    (ids, merge::ids(value, "keep_ids"))
}

struct Source {
    bot: Bot,
    engine: model::Bot,
    class: String,
    position: i64,
    /// Bot::Split#members: (asset, weight) that still trade here, heaviest first, the weights re-normalised.
    members: Vec<(i64, f64)>,
    /// Read fresh by `reason`.
    books: Option<(Subject, Metrics)>,
    /// `redeploy_offer`, read by `offers`.
    offer: Option<Dec>,
    /// D-SPLIT-LEGACY-SHADOW: (row, index of the child it goes to) for each legacy row one child's asset names.
    legacy: Vec<(i64, usize)>,
}

/// Why the split cannot go ahead: a key under errors.bots.split, with what it names. `Unrun`: this build's own refusal
/// (engine.write_refused), a venue it does not run the children on.
enum Reason { Plain(&'static str), Bot(&'static str, String), Busy(String), Unrun(&'static str) }

pub struct Split {
    ids: Vec<i64>,
    keep: Vec<i64>,
    /// The user's bots that are not deleted, of `ids`, in request order (`@split.bots`).
    found: Vec<i64>,
    bots: Vec<Source>,
    unrun: Option<&'static str>,
}

/// What loading the selection found.
pub enum Loaded { Split(Box<Split>), Unported(&'static str) }

/// Bots::RedeployJob still held by Solid Queue for this bot (Bot::Split#reinvesting?'s `active_job?`). None: the queue
/// can't be read (missing, unopenable, not Solid Queue's), which is never "no job".
fn queued_redeploy(app: &App, class: &str, id: i64) -> Option<bool> {
    let path = app.queue_path.as_deref().filter(|p| p.is_file())?;
    let queue = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    crate::engine::handover::active_job(&queue, "Bot::RedeployJob", &format!("gid://deltabadger/{class}/{id}")).ok()
}

/// `bot.redeploy_in_flight?` past the pending flag `refusal` already refuses, or a Yes Rails still holds: why it is not served.
fn redeploying(c: &Connection, app: &App, source: &model::Bot, class: &str) -> Result<Option<&'static str>, WebError> {
    let waiting: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id = ?1 AND transaction_type = 'REDEPLOY' AND status = 0 AND external_status IN (0, 1))",
                                    [source.id], |r| r.get(0))?;
    if waiting { return Ok(Some("a redeploy in progress")); }
    Ok(match queued_redeploy(app, class, source.id) {
        None => Some("the job queue can't be read"),
        Some(true) => Some("a redeploy in progress"),
        Some(false) => None,
    })
}

/// Bot::Split#members.
fn members(c: &Connection, bot: &Bot) -> Result<Vec<(i64, f64)>, WebError> {
    let quote = bot.setting("quote_asset_id").and_then(Value::as_i64);
    let mut s = c.prepare("SELECT asset_id, CAST(target_allocation AS REAL) FROM bot_index_assets WHERE bot_id = ?1 AND in_index = 1 ORDER BY target_allocation DESC")?;
    let rows = s.query_map([bot.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<f64>>(1)?.unwrap_or(0.0))))?.collect::<Result<Vec<_>, _>>()?;
    let mut pairs = vec![];
    for (asset, weight) in rows {
        let tradeable: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM tickers WHERE available = 1 AND trading_enabled = 1 AND exchange_id = ?1 AND quote_asset_id IS ?2 AND base_asset_id = ?3)",
                                          rusqlite::params![bot.exchange.id, quote, asset], |r| r.get(0))?;
        if weight > 0.0 && tradeable { pairs.push((asset, weight)); }
    }
    let total = crate::ruby::float_sum(&pairs.iter().map(|(_, w)| *w).collect::<Vec<_>>());
    Ok(pairs.into_iter().map(|(asset, weight)| (asset, weight / total)).collect())
}

impl Split {
    /// `Bot::Split.new(current_user, ids, keep_ids:)`: the selection is looked up first (`missing`, `none`), then each bot is
    /// asked what this build serves of it.
    pub fn load(c: &Connection, ctx: &Ctx, owner: i64, ids: Vec<i64>, keep: Vec<i64>) -> Result<Loaded, WebError> {
        let mut rows = vec![];
        for id in &ids {
            let row: Option<(Option<String>, i64, Option<i64>)> = c.query_row(
                "SELECT type, position, exchange_id FROM bots WHERE id = ?1 AND user_id = ?2 AND status <> 3",
                (id, owner), |r| Ok((r.get(0)?, r.get::<_, Option<i64>>(1)?.unwrap_or(0), r.get(2)?))).optional()?;
            if let Some(row) = row { rows.push((*id, row)); }
        }
        let mut split = Split { found: rows.iter().map(|(id, _)| *id).collect(), ids, keep, bots: vec![], unrun: None };
        if split.found.len() < split.ids.len() || split.found.is_empty() { return Ok(Loaded::Split(Box::new(split))); }
        let wash: Option<bool> = c.query_row("SELECT wash_sale_enabled FROM users WHERE id = ?1", [owner], |r| r.get(0))?;
        let (provider, _) = bots::market_data(c, &ctx.app)?;
        for (id, (class, position, exchange)) in rows {
            // The children are baskets that buy on the source's venue: one this build does not run them on is refused by name.
            if let Some(why) = exchange.map(|exchange| merge::unrun(c, exchange)).transpose()?.flatten() {
                split.unrun = Some(why);
                return Ok(Loaded::Split(Box::new(split)));
            }
            if let Some(reason) = super::refusal(c, id, wash, provider, For::Settings)? { return Ok(Loaded::Unported(reason)); }
            let Some(bot) = Bot::find(c, owner, id, For::Settings, ctx.locale)? else { continue };
            if let Some(reason) = bot.unrendered() { return Ok(Loaded::Unported(reason)); }
            let class = class.unwrap_or_default();
            let engine = model::load_bot(c, id)?;
            if let Some(why) = redeploying(c, &ctx.app, &engine, &class)? { return Ok(Loaded::Unported(why)); }
            let members = members(c, &bot)?;
            split.bots.push(Source { bot, engine, class, position, members, books: None, offer: None, legacy: vec![] });
        }
        Ok(Loaded::Split(Box::new(split)))
    }

    /// Bot::Split.eligible?, the redeploy left to the modal, and D-SPLIT-REVERSAL-PENDING.
    fn eligible(c: &Connection, source: &Source, now: At) -> Result<bool, WebError> {
        let members = source.bot.memberships.iter().filter(|m| m.in_index).count();
        Ok(merge::TYPES.contains(&source.class.as_str()) && source.bot.kind != Kind::Signal
            && !matches!(source.bot.status, BotStatus::Archived | BotStatus::Deleted | BotStatus::Executing)
            && !index_switch::busy(c, &source.engine, false, now.utc())? && members >= 2 && !merge::reversal_pending(&source.engine))
    }

    /// Bot::Split#reason, in its order; the books are read fresh. Then D-SPLIT-LEGACY-SHADOW's routing of each legacy row.
    fn reason(&mut self, c: &Connection, now: At) -> Result<Option<Reason>, WebError> {
        if self.found.len() < self.ids.len() { return Ok(Some(Reason::Plain("missing"))); }
        if self.found.is_empty() { return Ok(Some(Reason::Plain("none"))); }
        if let Some(why) = self.unrun { return Ok(Some(Reason::Unrun(why))); }
        if self.bots.len() < self.found.len() { return Ok(Some(Reason::Plain("missing"))); }
        for source in &self.bots {
            if !Self::eligible(c, source, now)? { return Ok(Some(Reason::Bot("unavailable", source.bot.label.clone()))); }
        }
        for source in &mut self.bots {
            let subject = Subject::load(c, source.bot.id).map_err(merge::fig)?;
            let books = walk::metrics(c, &subject, now).map_err(merge::fig)?;
            source.books = Some((subject, books));
        }
        fn books(source: &Source) -> &Metrics { &source.books.as_ref().expect("read above").1 }
        if let Some(source) = self.bots.iter().find(|source| books(source).walked.as_ref().is_some_and(|w| w.external_sales)) {
            return Ok(Some(Reason::Bot("external_sales", source.bot.label.clone())));
        }
        for source in &self.bots {
            if books(source).estimated_proceeds.to_d().map_err(|e| merge::fig(e.into()))?.is_positive() || unpriced_sale(c, source.bot.id)? {
                return Ok(Some(Reason::Bot("unpriced_sales", source.bot.label.clone())));
            }
        }
        if let Some(source) = self.bots.iter().find(|source| source.members.is_empty()) { return Ok(Some(Reason::Bot("nothing_to_buy", source.bot.label.clone()))); }
        for source in &mut self.bots {
            match legacy_routes(c, source)? {
                Some(routes) => source.legacy = routes,
                None => return Ok(Some(Reason::Bot("unavailable", source.bot.label.clone()))),
            }
        }
        Ok(None)
    }

    /// Bot::Split#offers: each bot's `redeploy_offer` on its fresh books. A bot this build cannot size an offer for is not
    /// served (Err), never offered a guessed zero.
    fn offers(&mut self, c: &Connection) -> Result<Result<(), &'static str>, WebError> {
        for source in &mut self.bots {
            let (subject, books) = source.books.as_ref().ok_or_else(|| super::data("offers before the books".into()))?;
            let members: HashSet<i64> = source.bot.memberships.iter().filter(|m| m.in_index).map(|m| m.asset.id).collect();
            match figure::redeploy_offer(c, subject, books, &members).map_err(merge::fig)? {
                Some(offer) => source.offer = Some(offer),
                None => return Ok(Err("a redeploy offer this build cannot size")),
            }
        }
        Ok(Ok(()))
    }

    fn offered(source: &Source) -> bool { source.offer.as_ref().is_some_and(Dec::is_positive) }

    /// Bot::Split#unanswered: the first bot whose proceeds wait for an answer, with every such bot's offer.
    fn unanswered(&self) -> Option<(Reason, Vec<(i64, Dec)>)> {
        let owed: Vec<&Source> = self.bots.iter().filter(|source| Self::offered(source) && !self.keep.contains(&source.bot.id)).collect();
        let first = owed.first()?;
        Some((Reason::Bot("proceeds", first.bot.label.clone()), owed.iter().map(|source| (source.bot.id, source.offer.clone().unwrap_or_else(Dec::zero))).collect()))
    }

    fn translate(locale: &str, reason: &Reason) -> String {
        let key = |k: &str| format!("errors.bots.split.{k}");
        match reason {
            Reason::Plain(k) => i18n::text(locale, &key(k), &[]),
            Reason::Bot(k, label) => i18n::text(locale, &key(k), &[("label", i18n::Arg::Text(label))]),
            Reason::Busy(exchange) => i18n::text(locale, &key("busy"), &[("exchange", i18n::Arg::Text(exchange))]),
            Reason::Unrun(why) => merge::write_refused(locale, why),
        }
    }

    /// A refusal as every caller reads it: the stable code, the user's words, and the offers still unanswered.
    fn refusal(locale: &str, reason: &Reason, offers: Vec<(i64, Dec)>) -> Refusal {
        let code = match reason {
            Reason::Plain(k) => k,
            Reason::Bot("proceeds", _) => "proceeds_unanswered",
            Reason::Bot(k, _) => k,
            Reason::Busy(_) => "busy",
            Reason::Unrun(_) => "write_refused",
        };
        Refusal { code, reason: Self::translate(locale, reason), offers }
    }
}

/// A sale Measurable#metrics estimates (`estimated_proceeds`): a submitted sell that executed units (a closed order's asked
/// units when it reported none) with no positive proceeds. This walk prices a REGULAR or REBALANCE one at price × units
/// (fill::special_sell, R4/R3) and estimates only a liquidation, so the rows are asked as Rails' walk asks them.
fn unpriced_sale(c: &Connection, bot: i64) -> Result<bool, WebError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id = ?1 AND status = 0 AND side = 1 \
                    AND coalesce(CAST(amount_exec AS REAL), CASE WHEN external_status = 2 THEN CAST(amount AS REAL) END, 0) > 0 \
                    AND NOT coalesce(CAST(quote_amount_exec AS REAL), 0) > 0)", [bot], |r| r.get(0))?)
}

/// D-SPLIT-LEGACY-SHADOW: where each row recorded without its asset goes. The candidates are every asset of the source's history
/// (its identified rows' and its memberships', in the index or exited), each bound for its own child or, without one, the
/// first. A row whose names (walk::listing_names) match candidates bound for one child goes there; one matching none stays
/// for the first; one matching candidates bound for different children is None, and the split is refused.
fn legacy_routes(c: &Connection, source: &Source) -> Result<Option<Vec<(i64, usize)>>, WebError> {
    let mut s = c.prepare("SELECT id, base FROM transactions WHERE bot_id = ?1 AND base_asset_id IS NULL ORDER BY id")?;
    let rows = s.query_map([source.bot.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))?.collect::<Result<Vec<_>, _>>()?;
    if rows.is_empty() { return Ok(Some(vec![])); }
    let mut s = c.prepare("SELECT base_asset_id FROM transactions WHERE bot_id = ?1 AND base_asset_id IS NOT NULL UNION SELECT asset_id FROM bot_index_assets WHERE bot_id = ?1")?;
    let candidates = s.query_map([source.bot.id], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    let listings = if candidates.is_empty() { vec![] } else { figures_db::listings(c, source.bot.exchange.id, &candidates).map_err(merge::fig)? };
    let child = |asset: i64| source.members.iter().position(|(id, _)| *id == asset).unwrap_or(0);
    let mut routes = vec![];
    for (row, base) in rows {
        let name = base.to_uppercase();
        let mut bound: Vec<usize> = listings.iter().filter(|(base, _, symbol)| walk::listing_names(base, symbol.as_deref()).contains(&name)).map(|(_, asset, _)| child(*asset)).collect();
        bound.sort_unstable();
        bound.dedup();
        match bound.as_slice() {
            [] => {}
            [one] => routes.push((row, *one)),
            _ => return Ok(None),
        }
    }
    Ok(Some(routes))
}

/// One "money from past sales" row of the modal.
struct Offer { id: i64, prompt: String, action: String }

#[derive(Template)]
#[template(path = "bots/splits/new.html")]
struct Modal<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    url: String,
    refusal: Option<String>,
    ids: Vec<i64>,
    offers: Vec<Offer>,
    stack_class: &'static str,
    pills: Vec<merge::Pill>,
}

/// bots/splits/new: the modal, refusal included, for the selection as it stands; Err when this build cannot size an offer.
pub fn modal(c: &Connection, ctx: &Ctx, split: &mut Split, csrf: &str) -> Result<Result<String, &'static str>, WebError> {
    let now = At::from_utc(ctx.now).ok_or_else(|| super::data("the split's time is out of range".into()))?;
    let refusal = split.reason(c, now)?.map(|reason| Split::translate(ctx.locale, &reason));
    let query: Vec<String> = split.found.iter().map(|id| format!("ids%5B%5D={id}")).collect();
    let url = if query.is_empty() { ctx.path("/bots/split/new") } else { format!("{}?{}", ctx.path("/bots/split/new"), query.join("&")) };
    let mut view = Modal { v: ctx, csrf, url, refusal, ids: split.found.clone(), offers: vec![], stack_class: "", pills: vec![] };
    if view.refusal.is_none() {
        if let Err(reason) = split.offers(c)? { return Ok(Err(reason)); }
        for source in split.bots.iter().filter(|source| Split::offered(source)) {
            let offer = source.offer.clone().unwrap_or_else(Dec::zero);
            let amount = figure::precision(&Num::Dec(offer), 2, true).map_err(merge::fig)?;
            let symbol = source.bot.quote_symbol().unwrap_or("").to_string();
            view.offers.push(Offer { id: source.bot.id, action: ctx.path(&format!("/bots/{}/redeploy", source.bot.id)), prompt: i18n::text(ctx.locale, "bot.split.proceeds", &[("label", i18n::Arg::Text(&source.bot.label)), ("amount", i18n::Arg::Text(&amount)), ("symbol", i18n::Arg::Text(&symbol))]) });
        }
        let mut assets: Vec<i64> = vec![];
        for (asset, _) in split.bots.iter().flat_map(|source| source.members.iter()) { if !assets.contains(asset) { assets.push(*asset); } }
        view.pills = merge::pills(c, &assets)?;
        view.stack_class = merge::stack_class(view.pills.len());
    }
    Ok(Ok(view.render()?))
}

/// A child: its id, its label and the asset it buys.
pub struct Child { pub id: i64, pub label: String, pub asset: i64 }

/// What a split made: the children in creation order and the sources it deleted.
pub struct Done { pub children: Vec<Child>, pub sources: Vec<i64> }

/// A refusal: a stable code (missing, none, unavailable, external_sales, unpriced_sales, nothing_to_buy, busy, write_refused,
/// proceeds_unanswered), the message the web shows, and for proceeds_unanswered each unanswered bot's offer.
pub struct Refusal { pub code: &'static str, pub reason: String, pub offers: Vec<(i64, Dec)> }

pub enum Performed { Done(Done), Refused(Refusal), Unported(&'static str) }

/// POST /bots/split (Bot::Split#perform!), in one write transaction: the reason and the answers re-checked on the rows they
/// read, the fence, each source split in request order, and the children as this build serves them; a refusal of any of them
/// rolls everything back. The caller runs `complete` after it returns Done.
pub fn perform(c: &Connection, ctx: &Ctx, owner: i64, ids: Vec<i64>, keep: Vec<i64>) -> Result<Performed, WebError> {
    match perform_inner(c, ctx, owner, ids, keep) {
        Err(e) if super::unreadable(&e) => Ok(Performed::Unported("a bot beyond the supported history, member or numeric bounds")),
        other => other,
    }
}

/// After the commit, in the same App::db job: the engine woken, every child's and source's figures and the owner's marked.
pub fn complete(app: &App, owner: i64, done: &Done) {
    app.wake_engine();
    for id in done.children.iter().map(|child| child.id).chain(done.sources.iter().copied()) { app.figure_service.mark(id); }
    app.figure_service.mark_owner(owner);
}

fn perform_inner(c: &Connection, ctx: &Ctx, owner: i64, ids: Vec<i64>, keep: Vec<i64>) -> Result<Performed, WebError> {
    let now = ctx.app.now();
    let at = At::from_utc(now).ok_or_else(|| super::data("the split's time is out of range".into()))?;
    let tx = model::immediate(c)?;
    let mut split = match Split::load(&tx, ctx, owner, ids, keep)? { Loaded::Split(split) => split, Loaded::Unported(reason) => return Ok(Performed::Unported(reason)) };
    if let Some(reason) = split.reason(&tx, at)? { return Ok(Performed::Refused(Split::refusal(ctx.locale, &reason, vec![]))); }
    if let Err(reason) = split.offers(&tx)? { return Ok(Performed::Unported(reason)); }
    if let Some((reason, offers)) = split.unanswered() { return Ok(Performed::Refused(Split::refusal(ctx.locale, &reason, offers))); }
    for source in &split.bots {
        if write::engine_holds(&tx, &source.engine)? { return Ok(Performed::Refused(Split::refusal(ctx.locale, &Reason::Busy(source.bot.exchange.name.clone()), vec![]))); }
    }
    let mut children = vec![];
    for source in &split.bots {
        // `rescue ActiveRecord::RecordInvalid`: a child Rails cannot save refuses the split as the first bot's `unavailable`.
        let Some(made) = split_one(&tx, ctx, owner, source, now)? else {
            tx.rollback()?;
            return Ok(Performed::Refused(Split::refusal(ctx.locale, &Reason::Bot("unavailable", split.bots[0].bot.label.clone()), vec![])));
        };
        children.extend(made);
    }
    // Each child as the dashboard and its page read it: a bot they would not serve is never committed.
    for child in &children {
        if let Some(reason) = merge::unserved(&tx, ctx, owner, child.id)? {
            tx.rollback()?;
            return Ok(Performed::Refused(Split::refusal(ctx.locale, &Reason::Unrun(reason), vec![])));
        }
    }
    if let Err(refusal) = eligibility::guard(&tx, &ctx.app.cipher, children.first().map(|child| child.id)) {
        tx.rollback()?;
        return Ok(Performed::Refused(Refusal { code: "write_refused", reason: merge::write_refused(ctx.locale, &refusal.reason()), offers: vec![] }));
    }
    tx.commit()?;
    Ok(Performed::Done(Done { children, sources: split.bots.iter().map(|source| source.bot.id).collect() }))
}

/// Bot::Split#split!: the children built and saved, every row moved as REGULAR, the memberships moved, the transient data
/// merged, the `split` activity logged on each child, and the source deleted with its run generation moved. None when a
/// child is invalid (Rails' `save!` raises).
fn split_one(tx: &Connection, ctx: &Ctx, owner: i64, source: &Source, now: DateTime<Utc>) -> Result<Option<Vec<Child>>, WebError> {
    let setting = |key: &str| source.bot.settings.get(key).cloned().unwrap_or(Value::Null);
    let amount = match setting("quote_amount") { Value::String(text) => text.trim().parse::<f64>().unwrap_or(0.0), other => other.as_f64().unwrap_or(0.0) };
    let exchange = source.bot.exchange.id;
    let mut children = vec![];
    for (asset, weight) in &source.members {
        // `quote_amount.to_f * weight`, stored as Oj writes a Float into the settings and read back as that number.
        let share = crate::figures::num::oj_float(amount * weight).parse::<f64>().unwrap_or(amount * weight);
        // Bot::Split#build_child!: the schedule and the one weight, the after_initialize defaults over them (no venue yet, so
        // no ticker), then `exchange=` with its limit-orders decorator.
        let config = json!({ "settings": { "quote_asset_id": setting("quote_asset_id"), "quote_amount": share, "interval": setting("interval"),
                                           "weighting": "manual", "allocations": { asset.to_string(): 1.0 } } });
        let mut built = wizard::create::build(tx, &config, ctx.locale)?;
        built.settings.retain(|_, value| !value.is_null());
        built.exchange_id = Some(exchange);
        let mut decorated = json!({ "exchange_id": exchange, "settings": Value::Object(std::mem::take(&mut built.settings)) });
        wizard::create::lock_limit_orders(tx, &mut decorated)?;
        built.settings = decorated["settings"].as_object().cloned().unwrap_or_default();
        built.set_members(tx, vec![*asset], ctx.locale)?;
        if !wizard::create::validate(tx, &built, ctx.locale)?.is_empty() { return Ok(None); }
        let label = built.label.clone();
        let id = wizard::create::insert_as(tx, owner, built, now, BotStatus::Stopped, source.position)?;
        children.push(Child { id, label, asset: *asset });
    }
    let Some(first) = children.first().map(|child| child.id) else { return Ok(Some(children)) };
    let from = source.bot.id;
    for child in &children {
        tx.execute("UPDATE transactions SET bot_id = ?1, transaction_type = 'REGULAR' WHERE bot_id = ?2 AND base_asset_id = ?3", (child.id, from, child.asset))?;
    }
    for (row, child) in &source.legacy {
        tx.execute("UPDATE transactions SET bot_id = ?1, transaction_type = 'REGULAR' WHERE id = ?2 AND bot_id = ?3", (children[*child].id, row, from))?;
    }
    tx.execute("UPDATE transactions SET bot_id = ?1, transaction_type = 'REGULAR' WHERE bot_id = ?2", (first, from))?;
    move_memberships(tx, from, &children, now)?;
    // merge_transient_data!: the child's last row and the source's resolved sales (`.presence`), then `.compact` drops every nil.
    let resolved: Vec<i64> = match source.engine.transient.get("liquidation_resolved_orders") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(items)) => items.iter().map(wizard::to_i).collect(),
        Some(other) => vec![wizard::to_i(other)],
    };
    for child in &children {
        let last: Option<i64> = tx.query_row("SELECT MAX(id) FROM transactions WHERE bot_id = ?1", [child.id], |r| r.get(0))?;
        let text: String = tx.query_row("SELECT transient_data FROM bots WHERE id = ?1", [child.id], |r| r.get(0))?;
        let mut transient: Map<String, Value> = serde_json::from_str(&text).map_err(|e| super::data(format!("split child transient: {e}")))?;
        transient.insert("merged_history_until_id".into(), json!(last));
        transient.insert("liquidation_resolved_orders".into(), if resolved.is_empty() { Value::Null } else { json!(resolved) });
        transient.retain(|_, value| !value.is_null());
        tx.execute("UPDATE bots SET transient_data = ?1 WHERE id = ?2", (Value::Object(transient).to_string(), child.id))?;
        model::log_activity(tx, child.id, "split", model::Level::Info, json!({ "source_id": from, "source_label": source.bot.label }), now)?;
    }
    // The source, deleted as the Delete button leaves a bot; its run generation moves, fencing an engine write in flight.
    tx.execute("UPDATE bots SET status = 3, stopped_at = ?1, \
                transient_data = json_set(transient_data, '$.run_generation', coalesce(json_extract(transient_data, '$.run_generation'), 0) + 1) WHERE id = ?2",
               (crate::codec::format_time(now), from))?;
    Ok(Some(children))
}

/// Bot::Split#move_memberships!: a child's own asset keeps the source's entry date; every other source row lands on the first
/// child as an exited row (its own ticker), once per asset.
fn move_memberships(tx: &Connection, from: i64, children: &[Child], now: DateTime<Utc>) -> Result<(), WebError> {
    let at = crate::codec::format_time(now);
    let first = children[0].id;
    let mut s = tx.prepare("SELECT id, asset_id, entered_at IS NOT NULL FROM bot_index_assets WHERE bot_id = ?1 ORDER BY id")?;
    let rows = s.query_map([from], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, bool>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    for (row, asset, entered) in rows {
        if let Some(child) = children.iter().find(|child| child.asset == asset) {
            if entered {
                tx.execute("UPDATE bot_index_assets SET entered_at = (SELECT entered_at FROM bot_index_assets WHERE id = ?1) \
                            WHERE id = (SELECT id FROM bot_index_assets WHERE bot_id = ?2 AND asset_id = ?3 ORDER BY id LIMIT 1)", (row, child.id, asset))?;
            }
        } else {
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM bot_index_assets WHERE bot_id = ?1 AND asset_id = ?2)", (first, asset), |r| r.get(0))?;
            if !exists {
                tx.execute("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, in_index, entered_at, exited_at, target_allocation, current_allocation, created_at, updated_at) \
                            SELECT ?1, asset_id, ticker_id, 0, entered_at, coalesce(exited_at, ?2), target_allocation, current_allocation, ?2, ?2 FROM bot_index_assets WHERE id = ?3",
                           (first, &at, row))?;
            }
        }
    }
    Ok(())
}
