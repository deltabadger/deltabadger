//! The new-bot wizard: BotsController#new (the bot-type chooser) and the order switch
//! (Bots::DcaSingleAssets::OrdersController), with the step order they share (Bots::Wizard::StepOrder and
//! Bots::Wizard::Navigable). The steps after the chooser are not served yet: their routes are named refusals.
//!
//! The wizard's state is `session[:bot_config]` (web::session), a JSON object with string keys as Rails'
//! cookie serializer holds it.
use super::layout::{self, Ctx, Page};
use super::shell::{self, Shell};
use super::{auth, App, WebError};
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

/// Navigable#current_variant: exchange-first unless the order switch chose asset-first (it writes only those two).
pub fn current_variant(config: Option<&Value>) -> Variant {
    if dig(config, &["flow"]).and_then(Value::as_str) == Some("asset_first") { Variant::AssetFirst } else { Variant::ExchangeFirst }
}

/// Navigable#step_complete?. The API key step is complete when the user's trading key for the chosen venue is
/// correct, which the database knows, not the session.
pub fn step_complete(c: &Connection, user_id: i64, config: Option<&Value>, step: Step) -> rusqlite::Result<bool> {
    match step {
        Step::Api => {
            let exchange = dig(config, EXCHANGE_KEY).and_then(|id| id.as_i64().or_else(|| id.as_str().and_then(|text| text.trim().parse().ok())));
            // `user.api_keys.find_by(exchange_id:, key_type: :trading)`, then `correct?`.
            let status: Option<i64> = c.query_row("SELECT status FROM api_keys WHERE user_id = ?1 AND exchange_id IS ?2 AND key_type = 0 ORDER BY id LIMIT 1",
                                                  rusqlite::params![user_id, exchange], |r| r.get(0)).optional()?;
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

/// Navigable#reset_downstream!: clear what `current` and the steps after it own. An emptied parent stays.
pub fn reset_downstream(config: &mut Value, order: &StepOrder, current: Step) {
    for key in order.reset_keys(current) {
        let (leaf, parents) = key.split_last().expect("a key path is never empty");
        if let Some(container) = parents.iter().try_fold(&mut *config, |value, key| value.get_mut(*key)).and_then(Value::as_object_mut) {
            container.remove(*leaf);
        }
    }
}

#[derive(Template)]
#[template(path = "bots/new.html")]
struct New<'a> {
    v: &'a Ctx,
    advanced: bool,
}

/// GET /bots/new: every new bot config starts fresh (`session[:bot_config] = {}`), the one GET of the wizard
/// that writes. The Signal tile shows only when the user enabled advanced bots.
pub async fn new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    ctx.session.lock().bot_config = Some(json!({}));
    let (inner, owner) = (app.clone(), user.clone());
    let (advanced, shell) = app.db(move |c| {
        let advanced = c.query_row("SELECT advanced_bots_enabled FROM users WHERE id = ?1", [owner.id], |r| r.get::<_, bool>(0))?;
        Ok((advanced, Shell::load(c, &inner, &owner)?))
    }).await?;
    let csrf = ctx.csrf_token();
    let body = New { v: &ctx, advanced }.render()?;
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
    fn step_order_matches_rails() {
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
