use super::model::{string, Rule};
use super::*;
use crate::{
    ruby::{self, BigDec},
    web::{
        bot::{
            search::{self, AssetPage, Basket, Item, Side},
            Asset,
        },
        colors,
        i18n::{escape, Arg},
        settings::{key_check, keys},
        wizard::{
            exchanges::{Table, Venue},
            keys::{venue, FullPage},
        },
    },
};
use rusqlite::OptionalExtension;
use serde_json::Value;
#[derive(Template)]
#[template(path = "rules/pick_exchange.html")]
struct Pick<'a> {
    v: &'a Ctx,
    progress: String,
    table: Table,
}
#[derive(Template)]
#[template(path = "rules/sentence.html")]
struct Sentence<'a> {
    v: &'a Ctx,
    search: bool,
    asset: Option<&'a Asset>,
    color: String,
    asset_url: String,
    exchange_url: String,
    exchange: &'a str,
    address: bool,
}
#[derive(Template)]
#[template(path = "rules/pick_asset.html")]
struct Assets {
    progress: String,
    sentence: String,
    action: String,
    csrf: String,
    items: Vec<Item>,
    field_name: &'static str,
    next: Option<(String, String)>,
}
#[derive(Template)]
#[template(path = "rules/settings.html")]
struct Settings<'a> {
    v: &'a Ctx,
    progress: String,
    action: String,
    csrf: String,
    preview: String,
}
#[derive(Template)]
#[template(path = "rules/preview.html")]
struct Preview<'a> {
    v: &'a Ctx,
    r: &'a Rule,
    color: String,
    asset_url: String,
    exchange_url: String,
    address_url: String,
    threshold: String,
    listed: bool,
    display: String,
    options: String,
    truncated: String,
    infos: Vec<(bool, String)>,
}

fn to(ctx: &Ctx, step: &str) -> Response {
    layout::redirect(StatusCode::FOUND, &ctx.path(&format!("{BASE}/{step}/new")))
}
fn config(ctx: &Ctx) -> Value {
    ctx.session
        .lock()
        .withdrawal_rule_config
        .clone()
        .unwrap_or_else(|| json!({}))
}
fn set(ctx: &Ctx, value: Value) {
    ctx.session.lock().withdrawal_rule_config = Some(value)
}
fn progress(ctx: &Ctx, step: &str, n: i64) -> Result<String, WebError> {
    crate::web::wizard::progress_bar(ctx.t(&format!("bot.setup.progress_steps.{step}")), n)
}
async fn full(app: &App, ctx: &Ctx, content: String) -> Result<Response, WebError> {
    page(
        app,
        ctx,
        FullPage {
            close_url: ctx.path("/rules"),
            content: &content,
        }
        .render()?,
    )
    .await
}
fn sentence(
    ctx: &Ctx,
    exchange: &str,
    asset: Option<&Asset>,
    step: &str,
) -> Result<String, WebError> {
    Ok(Sentence {
        v: ctx,
        search: step == "pick_asset",
        asset,
        color: colors::pill_color(asset.and_then(|a| a.color.as_deref())).unwrap_or_default(),
        asset_url: ctx.path(&format!("{BASE}/pick_asset/new")),
        exchange_url: ctx.path(&format!("{BASE}/pick_exchange/new")),
        exchange,
        address: step == "add_address",
    }
    .render()?)
}
async fn temporary(app: &App, ctx: &Ctx, config: Value) -> Result<Option<Rule>, WebError> {
    let user = ctx.user().expect("authenticated").id;
    app.db(move|c|{
        let Some(exchange)=venue(c,config.get("exchange_id").map(crate::web::wizard::to_i))?else{return Ok(None)};
        let Some(asset)=Asset::find(c,crate::web::wizard::to_i(&config["asset_id"]))?else{return Ok(None)};
        let (fee,chains):(Option<String>,Option<String>)=c.query_row("SELECT withdrawal_fee,withdrawal_chains FROM exchange_assets WHERE exchange_id=?1 AND asset_id=?2",
            (exchange.id,asset.id),|r|Ok((r.get(0)?,r.get(1)?))).optional()?.unwrap_or_default();
        Ok(Some(Rule{id:0,user_id:user,status:0,exchange,asset,address:string(&config["address"]),settings:config,
            fee:fee.and_then(|f|BigDec::parse(&f).ok()),chains:chains.and_then(|s|serde_json::from_str(&s).ok()).unwrap_or_default(),balance:None,unconfirmed:None}))
    }).await
}

async fn exchange(app: &App, ctx: &Ctx) -> Result<Response, WebError> {
    if ctx.method == axum::http::Method::POST {
        if let Some(id) = ctx
            .params
            .form("bots_dca_single_asset[exchange_id]")
            .filter(|s| !ruby::blank(s))
        {
            let n = crate::web::bot::search::to_i(id);
            let allowed = app
                .db(move |c| {
                    Ok(venue(c, Some(n))?
                        .is_some_and(|v| crate::venue::withdrawal::Kind::of(&v.class).is_some()))
                })
                .await?;
            if !allowed {
                return Ok(to(ctx, "pick_exchange"));
            }
            let mut conf = config(ctx);
            conf["exchange_id"] = json!(id);
            conf.as_object_mut().unwrap().remove("asset_id");
            set(ctx, conf);
            return Ok(to(ctx, "add_api_key"));
        }
        return Ok(to(ctx, "pick_exchange"));
    }
    set(ctx, json!({}));
    let v = ctx.clone();
    let content = app
        .db(move |c| {
            let venues = search::available_exchanges(
                c,
                &Basket {
                    exchange_id: None,
                    quote_asset_id: None,
                    base_asset_ids: vec![],
                },
                Some(None),
            )?;
            let venues = crate::web::wizard::exchanges::ordered(venues, None)
                .into_iter()
                .filter(|e| crate::venue::withdrawal::Kind::of(&e.class).is_some())
                .map(|e| Venue {
                    id: e.id,
                    mark: crate::web::bot::exchange_wordmark(&e.name_id()),
                    unavailable: false,
                })
                .collect();
            Ok(Pick {
                v: &v,
                progress: progress(&v, "pick_exchange", 20)?,
                table: Table {
                    action: v.path(&format!("{BASE}/pick_exchange")),
                    csrf: v.csrf_token(),
                    param_name: "bots_dca_single_asset[exchange_id]".into(),
                    venues,
                    unavailable_note: v.t("bot.hyperliquid_trading_unavailable"),
                },
            }
            .render()?)
        })
        .await?;
    full(app, ctx, content).await
}

async fn assets(
    app: &App,
    ctx: &Ctx,
    exchange: crate::web::wizard::keys::Venue,
) -> Result<Response, WebError> {
    if ctx.method == axum::http::Method::POST {
        if let Some(id) = ctx
            .params
            .form("bots_dca_single_asset[asset_id]")
            .filter(|s| !ruby::blank(s))
        {
            let mut conf = config(ctx);
            conf["asset_id"] = json!(id);
            for key in ["address", "address_name", "address_tag", "network"] {
                conf.as_object_mut().unwrap().remove(key);
            }
            set(ctx, conf);
            return Ok(to(ctx, "add_api_key"));
        }
        return Ok(to(ctx, "pick_asset"));
    }
    let v = ctx.clone();
    let (content, partial) = app
        .db(move |c| {
            let scope = Basket {
                exchange_id: Some(exchange.id),
                quote_asset_id: None,
                base_asset_ids: vec![],
            };
            let all = search::filtered_rows(c, &scope, Side::Base, v.params.query("query"))?;
            let offset = v.params.query("offset").map_or(0, search::to_i);
            let next_offset = offset.saturating_add(search::PAGE_SIZE as i64);
            let next = (all.len() as i64 > next_offset).then(|| {
                (
                    format!("assets-page-{next_offset}"),
                    format!(
                        "{}?{}",
                        v.params.full_path,
                        search::next_page_query(
                            v.params.fullpath.split_once('?').map_or("", |(_, q)| q),
                            next_offset
                        )
                    ),
                )
            });
            let mut rows = search::page_of(all, offset);
            search::attach_exchanges(c, &scope, Side::Base, &mut rows)?;
            let items = search::items(rows)?;
            if offset > 0 {
                return Ok((
                    AssetPage {
                        frame_id: format!("assets-page-{offset}"),
                        field_name: "bots_dca_single_asset[asset_id]".into(),
                        items,
                        next,
                    }
                    .render()?,
                    true,
                ));
            }
            Ok((
                Assets {
                    progress: progress(&v, "pick_asset", 60)?,
                    sentence: sentence(&v, &exchange.name, None, "pick_asset")?,
                    action: v.path(&format!("{BASE}/pick_asset")),
                    csrf: v.csrf_token(),
                    items,
                    field_name: "bots_dca_single_asset[asset_id]",
                    next,
                }
                .render()?,
                false,
            ))
        })
        .await?;
    if partial {
        Ok(layout::html(StatusCode::OK, content))
    } else {
        full(app, ctx, content).await
    }
}
async fn key_card(
    app: &App,
    ctx: &Ctx,
    exchange: &crate::web::wizard::keys::Venue,
) -> Result<String, WebError> {
    let (inner, v, e) = (app.clone(), ctx.clone(), exchange.clone());
    app.db(move|c|{
        let instructions=keys::key_instructions(c,&inner,&v,&e.class,&e.name,1)?;
        keys::card(&v,&v.csrf_token(),v.path(&format!("{BASE}/add_api_key?exchange_id={}",e.id)),
            "data-controller=\"form--label-animations form--html5-validations\" data-turbo-frame=\"modal_content\"",
            &e.name,&e.class,false,false,instructions)
    }).await
}
async fn next(app: &App, ctx: &Ctx, via_address: bool) -> Result<Response, WebError> {
    let Some(rule) = temporary(app, ctx, config(ctx)).await? else {
        return Ok(to(ctx, "pick_asset"));
    };
    let Some(list) = reads::listing(app, &rule, false).await? else {
        return Ok(to(ctx, "add_address"));
    };
    let Some(first) = list.entries.iter().find(|e| e.refused.is_none()) else {
        return Ok(to(ctx, "add_address"));
    };
    let mut conf = config(ctx);
    if !via_address || conf["address"].is_null() {
        conf["address"] = json!(first.address);
    }
    if via_address && conf["address_name"].is_null() {
        conf["address_name"] = json!(first.key);
    }
    set(ctx, conf);
    Ok(to(ctx, "confirm_settings"))
}
async fn keys(
    app: &App,
    ctx: &Ctx,
    exchange: crate::web::wizard::keys::Venue,
) -> Result<Response, WebError> {
    let (inner, id, user) = (
        app.clone(),
        exchange.id,
        ctx.user().expect("authenticated").id,
    );
    let key = app
        .db(move |c| keys::Key::read(c, &inner, user, id, 1))
        .await?;
    if ctx.method == axum::http::Method::POST {
        if !ctx
            .params
            .form
            .iter()
            .any(|(k, _)| k.starts_with("api_key["))
        {
            return Ok(layout::bad_request());
        }
        let message = match keys::submit(
            app,
            ctx,
            user,
            id,
            &exchange.class,
            &exchange.name,
            1,
            key,
        )
        .await?
        {
            keys::Submitted::Correct => {
                let response = next(app, ctx, false).await?;
                let target = response
                    .headers()
                    .get(axum::http::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("/rules");
                return Ok(crate::web::wizard::stream(
                    StatusCode::OK,
                    turbo::redirect(target),
                ));
            }
            keys::Submitted::Incorrect => ctx.t("errors.incorrect_api_key_permissions"),
            keys::Submitted::Permissions(missing, forbidden) => key_check::permission_message(
                ctx.locale,
                &crate::tracker::rows::name_id(&exchange.class),
                &exchange.name,
                1,
                &missing,
                &forbidden,
            ),
            keys::Submitted::Failed => ctx.t("errors.api_key_permission_validation_failed"),
            keys::Submitted::AccountMode(message) => ctx.t(message),
            keys::Submitted::Refused(reason) => i18n::text(
                ctx.locale,
                "engine.write_refused",
                &[("reason", Arg::Text(&reason))],
            ),
        };
        return alert(ctx, message, false);
    }
    let mut correct = key.status == 1;
    if !correct
        && key.key.as_deref().is_some_and(|s| !ruby::blank(s))
        && key.secret.as_deref().is_some_and(|s| !ruby::blank(s))
    {
        if let Some(id) = key.id {
            if let Some(reason) = keys::revalidate(app, id).await? {
                return alert(ctx, reason, false);
            }
            correct = app
                .db(move |c| {
                    Ok(
                        c.query_row("SELECT status=1 FROM api_keys WHERE id=?1", [id], |r| {
                            r.get::<_, bool>(0)
                        })
                        .optional()?
                        .unwrap_or(false),
                    )
                })
                .await?;
        }
    }
    if correct {
        return next(app, ctx, false).await;
    }
    let rule = temporary(app, ctx, config(ctx)).await?;
    let content = format!(
        "<div class=\"bot-creation-layout\">\n{}\n{}\n{}\n</div>",
        progress(ctx, "api", 40)?,
        sentence(
            ctx,
            &exchange.name,
            rule.as_ref().map(|r| &r.asset),
            "add_api_key"
        )?,
        key_card(app, ctx, &exchange).await?
    );
    full(app, ctx, content).await
}
async fn addresses(app: &App, ctx: &Ctx, rule: Rule) -> Result<Response, WebError> {
    let list = reads::listing(app, &rule, ctx.method == axum::http::Method::POST).await?;
    if ctx.method == axum::http::Method::POST {
        let mut conf = config(ctx);
        conf["address"] = json!(ctx.params.form("address").unwrap_or_default());
        conf["address_tag"] = ctx
            .params
            .form("address_tag")
            .filter(|s| !ruby::blank(s))
            .map_or(Value::Null, |s| json!(s));
        let mut candidate = rule;
        candidate.address = string(&conf["address"]);
        candidate.settings = conf.clone();
        if list
            .as_ref()
            .and_then(|l| reads::select(&candidate, l).ok())
            .is_none()
        {
            return Ok(to(ctx, "add_address"));
        }
        set(ctx, conf);
        return Ok(to(ctx, "confirm_settings"));
    }
    let (inner, user, id) = (app.clone(), rule.user_id, rule.exchange.id);
    let correct = app
        .db(move |c| Ok(keys::Key::read(c, &inner, user, id, 1)?.status == 1))
        .await?;
    if !correct {
        return Ok(to(ctx, "add_api_key"));
    }
    if let Some(first) = list
        .as_ref()
        .and_then(|l| l.entries.iter().find(|e| e.refused.is_none()))
    {
        let mut conf = config(ctx);
        if conf["address"].is_null() {
            conf["address"] = json!(first.address)
        }
        if conf["address_name"].is_null() {
            conf["address_name"] = json!(first.key)
        }
        set(ctx, conf);
        return Ok(to(ctx, "confirm_settings"));
    }
    let content = if list.is_some() {
        format!("<div class=\"set-api widget\">\n<div class=\"set-api__form\">\n<p>{}</p>\n<a data-turbo-frame=\"modal_content\" class=\"button button--sky button--large\" href=\"{}\">{}</a>\n</div>\n</div>",
            i18n::t(ctx.locale,"rules.add_address.no_addresses_html",&[("symbol",Arg::Text(rule.asset.symbol())),("exchange",Arg::Text(&rule.exchange.name))]),
            escape(&ctx.path(&format!("{BASE}/add_address/new"))),ctx.t("button.check_again"))
    } else {
        format!(
            "<p class=\"text-center\">{}</p>\n{}",
            i18n::t(
                ctx.locale,
                "rules.add_address.fetch_failed_html",
                &[("exchange", Arg::Text(&rule.exchange.name))]
            ),
            key_card(app, ctx, &rule.exchange).await?
        )
    };
    full(
        app,
        ctx,
        format!(
            "<div class=\"bot-creation-layout\">\n{}\n{}\n{content}\n</div>",
            progress(ctx, "address", 80)?,
            sentence(ctx, &rule.exchange.name, Some(&rule.asset), "add_address")?
        ),
    )
    .await
}

fn preview(ctx: &Ctx, r: &Rule, list: Option<&reads::Listing>) -> Result<String, WebError> {
    let threshold = r.threshold();
    let fee = r.fee_amount();
    let amount = BigDec::parse(&r.value("min_amount", "0.1")).unwrap_or_else(|_| BigDec::zero());
    let max =
        BigDec::parse(&r.value("max_fee_percentage", "0.5")).unwrap_or_else(|_| BigDec::zero());
    let fraction = BigDec::parse(&r.percentage())
        .ok()
        .and_then(|n| n.div(&BigDec::from_i64(100)))
        .unwrap_or_else(BigDec::zero);
    let min = fee
        .div(&max.div(&BigDec::from_i64(100)).unwrap_or_else(BigDec::zero))
        .and_then(|v| v.div(&fraction))
        .map(|v| v.round(8))
        .unwrap_or_else(BigDec::zero);
    let pct = fee
        .div(&amount)
        .map(|n| &n * &BigDec::from_i64(100))
        .unwrap_or_else(BigDec::zero);
    let pct_text = if fee.is_positive() && amount.is_positive() {
        format!("{}%",format::float_to_s(format::float_round(pct.to_f(),2)))
    } else { "0%".into() };
    let pct_text = if pct > BigDec::one() {
        format!("<span class=\"text-danger\">{pct_text}</span>")
    } else {
        pct_text
    };
    let infos = vec![
        (
            threshold == "min_amount" && fee.is_positive() && amount.is_positive(),
            i18n::t(
                ctx.locale,
                "rules.tile.fixed_amount_fee_html",
                &[
                    ("fee", Arg::Text(&fee.to_s_f())),
                    ("symbol", Arg::Text(r.asset.symbol())),
                    ("fee_pct", Arg::Html(&pct_text)),
                ],
            ),
        ),
        (
            threshold == "min_amount" && (!fee.is_positive() || !amount.is_positive()),
            i18n::t(
                ctx.locale,
                "rules.tile.fixed_amount_info",
                &[
                    (
                        "amount",
                        Arg::Text(
                            &r.minimum()
                                .map(|n| n.to_s_f())
                                .unwrap_or_else(|| r.value("min_amount", "0.1")),
                        ),
                    ),
                    ("symbol", Arg::Text(r.asset.symbol())),
                ],
            ),
        ),
        (
            threshold == "fee_percentage" && fee.is_positive() && max.is_positive(),
            i18n::t(
                ctx.locale,
                "rules.tile.min_amount_info_html",
                &[
                    ("fee", Arg::Text(&fee.to_s_f())),
                    ("symbol", Arg::Text(r.asset.symbol())),
                    ("amount", Arg::Text(&min.to_s_f())),
                ],
            ),
        ),
        (
            threshold == "fee_percentage" && r.fee.is_some() && fee.is_zero(),
            i18n::t(
                ctx.locale,
                "rules.tile.no_fee_info",
                &[("exchange", Arg::Text(&r.exchange.name))],
            ),
        ),
    ];
    let display = list
        .and_then(|l| {
            l.entries.iter().find(|e| {
                e.address == r.address && r.settings["address_name"].as_str()
                    .filter(|key| !ruby::blank(key)).is_none_or(|key| e.key.as_deref() == Some(key))
            })
        })
        .and_then(|e| e.key.clone())
        .unwrap_or_else(|| r.address.clone());
    // The preview's hand-written options use a boolean selected attribute, unlike form-builder selects.
    let options = list
        .map(|l| view::options(l, r).replace("selected=\"selected\"", "selected"))
        .unwrap_or_default();
    Ok(Preview {
        v: ctx,
        r,
        color: colors::pill_color(r.asset.color.as_deref()).unwrap_or_default(),
        asset_url: ctx.path(&format!("{BASE}/pick_asset/new")),
        exchange_url: ctx.path(&format!("{BASE}/pick_exchange/new")),
        address_url: ctx.path(&format!("{BASE}/add_address/new")),
        threshold,
        listed: list.is_some_and(|l| !l.entries.is_empty()),
        display,
        options,
        truncated: if r.address.chars().count() > 16 {
            r.address.chars().take(13).collect::<String>() + "..."
        } else {
            r.address.clone()
        },
        infos,
    }
    .render()?)
}

async fn settings(app: &App, ctx: &Ctx, mut rule: Rule) -> Result<Response, WebError> {
    let is_preview = ctx.params.route_path.ends_with("/preview");
    let is_create = ctx.method == axum::http::Method::POST && !is_preview;
    let posted = match ctx
        .params
        .form("destination")
        .map(reads::Identity::parse)
        .transpose()
    {
        Ok(identity) => identity,
        Err(_) => return Ok(to(ctx, "add_address")),
    };
    if let Some(identity) = &posted {
        let kind =
            crate::venue::withdrawal::Kind::of(&rule.exchange.class).expect("withdrawal venue");
        identity.apply(&mut rule.address, &mut rule.settings, kind);
        rule.settings["address"] = json!(rule.address);
    }
    if is_create {
        let patch=service::Patch {
            address:Some(rule.address.clone()),settings:rule.settings.clone(),destination:posted,
        };
        match service::call(app,rule.user_id,service::Command::Create {exchange_id:rule.exchange.id,asset_id:rule.asset.id,patch}).await? {
            Err(e) if e.code=="address_not_listed" || e.code=="credential_changed"=>return Ok(to(ctx,"add_address")),
            Err(e) if e.code=="rule_exists"=>return alert(ctx,i18n::t(ctx.locale,"errors.withdrawal_rule_already_exists",
                &[("asset_symbol",Arg::Text(rule.asset.symbol())),("exchange_name",Arg::Text(&rule.exchange.name))]),false),
            Err(e)=>return alert(ctx,e.reason,false),
            Ok(_)=>{}
        }
        ctx.session.lock().withdrawal_rule_config=None;
        return Ok(crate::web::wizard::stream(StatusCode::OK,turbo::redirect(&ctx.path("/rules"))));
    }
    let list=reads::listing(app,&rule,false).await?;
    if let Some(identity)=&posted {
        if !list.as_ref().is_some_and(|l|identity.listed(&rule,l)) {return Ok(to(ctx,"add_address"))}
    }
    if rule.exchange.class=="Exchanges::Mexc" {
        if let Some(destination)=list.as_ref().and_then(|l|reads::select(&rule,l).ok()) {
            reads::store(&mut rule.settings,&destination);
        }
    }
    if posted.is_some() {set(ctx,rule.settings.clone());}
    if let Some(list) = &list {
        if !list.entries.is_empty() {
            let at_address = list
                .entries
                .iter()
                .filter(|e| e.address == rule.address)
                .collect::<Vec<_>>();
            if at_address.is_empty() {
                let mut conf = config(ctx);
                conf.as_object_mut().unwrap().remove("address");
                conf.as_object_mut().unwrap().remove("address_name");
                set(ctx, conf);
                return Ok(to(ctx, "add_address"));
            }
            if let [selected] = at_address.as_slice() {
                let mut conf = config(ctx);
                if conf["address_name"].is_null() {
                    conf["address_name"] = json!(selected.key);
                    set(ctx, conf);
                }
            }
        }
    }
    reads::refresh_fee(app, &mut rule, list.as_ref()).await?;
    // The preview has defaults; only values actually submitted are saved in the wizard session.
    if rule.settings["threshold_type"].is_null() {
        rule.settings["threshold_type"] = json!(if rule.fee.is_some() {
            "fee_percentage"
        } else {
            "min_amount"
        })
    }
    rule.settings["withdrawal_percentage"] = json!(rule.percentage());
    for (key, default) in [("max_fee_percentage", "0.5"), ("min_amount", "0.1")] {
        if rule.settings[key].is_null() {
            rule.settings[key] = json!(default)
        }
    }
    let preview = preview(ctx, &rule, list.as_ref())?;
    if is_preview {
        return Ok(crate::web::wizard::stream(
            StatusCode::OK,
            turbo::stream("replace", "rule-preview", &(preview + "\n")),
        ));
    }
    full(
        app,
        ctx,
        Settings {
            v: ctx,
            progress: progress(ctx, "settings", 100)?,
            action: ctx.path(&format!("{BASE}/confirm_settings")),
            csrf: ctx.csrf_token(),
            preview,
        }
        .render()?,
    )
    .await
}

pub async fn step(
    State(app): State<App>,
    Extension(ctx): Extension<Ctx>,
) -> Result<Response, WebError> {
    if ctx.user().is_none() {
        return Ok(auth::unauthenticated(&ctx));
    }
    let suffix = ctx.params.route_path.strip_prefix(BASE).unwrap_or_default();
    let name = suffix
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();
    if name == "pick_exchange" {
        return exchange(&app, &ctx).await;
    }
    if name == "confirm_settings" && ctx.method == axum::http::Method::POST {
        let mut conf = config(&ctx);
        for field in [
            "withdrawal_percentage",
            "threshold_type",
            "max_fee_percentage",
            "min_amount",
            "max_interval",
            "network",
            "address",
            "address_tag",
            "address_name",
        ] {
            if let Some(value) = ctx.params.form(field) {
                conf[field] = json!(value)
            }
        }
        set(&ctx, conf)
    }
    let conf = config(&ctx);
    let exchange_id = if name == "add_api_key" && ctx.method == axum::http::Method::POST {
        ctx.params
            .form("exchange_id")
            .or_else(|| ctx.params.query("exchange_id"))
            .map(search::to_i)
    } else {
        conf.get("exchange_id").map(crate::web::wizard::to_i)
    };
    let exchange = app.db(move |c| venue(c, exchange_id)).await?;
    let Some(exchange) = exchange else {
        return Ok(to(&ctx, "pick_exchange"));
    };
    if crate::venue::withdrawal::Kind::of(&exchange.class).is_none() {
        return Ok(to(&ctx, "pick_exchange"));
    }
    if name == "pick_asset" {
        return assets(&app, &ctx, exchange).await;
    }
    if name == "add_api_key" {
        return keys(&app, &ctx, exchange).await;
    }
    let Some(rule) = temporary(&app, &ctx, conf).await? else {
        return Ok(to(&ctx, "pick_asset"));
    };
    if name == "add_address" {
        return addresses(&app, &ctx, rule).await;
    }
    if rule.address.trim().is_empty() {
        return Ok(to(&ctx, "add_address"));
    }
    settings(&app, &ctx, rule).await
}
