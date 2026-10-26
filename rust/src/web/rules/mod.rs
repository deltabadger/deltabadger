//! Withdrawal pages (W1). Rails' controllers and views are pinned by tests/pages.rs.
//! ponytail: W2 moves these minimal web writes into the shared rules service.
pub mod model;
mod reads;
pub mod view;
mod wizard;

use crate::web::{
    auth, cable, flash, format, i18n,
    layout::{self, Ctx, Page},
    shell::{self, Shell},
    turbo, App, WebError,
};
use askama::Template;
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::Response,
};
use rusqlite::Connection;
use serde_json::json;
pub use wizard::step;

pub const BASE: &str = "/rules/withdrawals";

pub async fn page(app: &App, ctx: &Ctx, body: String) -> Result<Response, WebError> {
    let user = ctx.user().cloned().expect("authenticated");
    let (inner, owner) = (app.clone(), user.clone());
    let shell = app.db(move |c| Shell::load(c, &inner, &owner)).await?;
    shell::application(
        ctx,
        &ctx.csrf_token(),
        &user,
        &shell,
        Page {
            status: StatusCode::OK,
            body,
            flash_now: vec![],
        },
    )
}
pub fn alert(ctx: &Ctx, message: String, refresh: bool) -> Result<Response, WebError> {
    let flash = flash::render(&flash::take(
        &ctx.session,
        &[(flash::ALERT, view::safe_message(&message))],
    ))?;
    Ok(crate::web::wizard::stream(
        StatusCode::UNPROCESSABLE_ENTITY,
        turbo::prepend_flash(flash.trim_end()) + if refresh { turbo::refresh() } else { "" },
    ))
}
#[derive(Template)]
#[template(path = "rules/index.html")]
struct Index<'a> {
    v: &'a Ctx,
    source: String,
    empty: bool,
    add: String,
    tiles: String,
    logs: String,
}

pub async fn index(
    State(app): State<App>,
    Extension(ctx): Extension<Ctx>,
) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let (inner, v) = (app.clone(), ctx.clone());
    let (rules,logs,empty_logs) = app.db(move |c| {
        let ids = c.prepare("SELECT id FROM rules WHERE user_id=?1 AND type='Rules::Withdrawal' AND status<>3 ORDER BY id")?
            .query_map([user.id],|r| r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
        let rules = ids.into_iter().map(|id| model::load(c,&inner,user.id,id)).collect::<Result<Vec<_>,_>>()?.into_iter().flatten().collect::<Vec<_>>();
        let logs = c.prepare("SELECT l.status,l.message,l.created_at FROM rule_logs l JOIN rules r ON r.id=l.rule_id WHERE r.user_id=?1 AND l.status<>0 ORDER BY l.created_at DESC LIMIT 50")?
            .query_map([user.id],|r| Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
        let empty = logs.is_empty();
        let mut html = String::from("<div class=\"widget widget--table widget--table--rules\" data-controller=\"table-fit\">
");
        if empty {
            html += &format!("<p style=\"padding: 9rem 2rem; text-align: center;\">{}</p>",v.t("rules.logs.empty"));
        } else {
            html += &format!("<table class=\"table\">
<thead>
<tr>
<th>{}</th>
<th>{}</th>
<th>{}</th>
</tr>
</thead>
<tbody>
",v.t("rules.logs.date"),v.t("rules.logs.status"),v.t("rules.logs.message"));
            for (status,message,at) in logs {
                let status = match status {1=>"success",2=>"failed",_=>"transient"};
                let at = crate::codec::parse_time(&at).map_err(|_| WebError::Config("rule log time unreadable".into()))?;
                html += &format!("<tr>
<td class=\"table__when\">{} <small>{}</small></td>
<td><span class=\"rule-log-status rule-log-status--{status}\">{status}</span></td>
<td class=\"table__sentence\">{}</td>
</tr>
",
                    format::table_date(at,&user.time_zone),format::table_clock(at,&user.time_zone,v.locale), i18n::escape(&view::safe_message(&message.unwrap_or_default())));
            }
            html += "</tbody>
</table>
";
        }
        html += "
</div>";
        Ok((rules,html,empty))
    }).await?;
    let mut tiles = String::new();
    for rule in &rules {
        let listing = if rule.status == 2 {
            reads::listing(&app, rule, false).await?
        } else {
            None
        };
        tiles += &view::tile(&ctx, rule, listing.as_ref())?;
        tiles.push('\n');
    }
    let body = Index {
        v: &ctx,
        source: cable::stream_source(
            &app.keys.streams,
            &format!(
                "user_{}:rule_updates",
                ctx.user().expect("authenticated").id
            ),
        ),
        empty: rules.is_empty() && empty_logs,
        add: ctx.path(&format!("{BASE}/pick_exchange/new")),
        tiles,
        logs,
    }
    .render()?;
    page(&app, &ctx, body).await
}

fn validation(rule: &model::Rule, ctx: &Ctx) -> Option<String> {
    if rule.status != 1 {
        return None;
    }
    let mut errors = vec![];
    for (key, label, cap) in [
        ("withdrawal_percentage", "Withdrawal percentage", true),
        (
            if rule.text("threshold_type") == "min_amount" {
                "min_amount"
            } else {
                "max_fee_percentage"
            },
            if rule.text("threshold_type") == "min_amount" {
                "Min amount"
            } else {
                "Max fee percentage"
            },
            rule.text("threshold_type") != "min_amount",
        ),
    ] {
        let value = if key == "withdrawal_percentage" {
            rule.percentage()
        } else {
            rule.text(key)
        };
        if value.trim().is_empty() && key != "withdrawal_percentage" {
            errors.push(format!(
                "{label} {}",
                i18n::text(ctx.locale, "errors.messages.blank", &[])
            ));
        }
        match crate::ruby::BigDec::parse(&value) {
            Err(_) => errors.push(format!(
                "{label} {}",
                i18n::text(ctx.locale, "errors.messages.not_a_number", &[])
            )),
            Ok(n) if !n.is_positive() => errors.push(format!(
                "{label} {}",
                i18n::text(
                    ctx.locale,
                    "errors.messages.greater_than",
                    &[("count", i18n::Arg::Count(0))]
                )
            )),
            Ok(n) if cap && n > crate::ruby::BigDec::from_i64(100) => errors.push(format!(
                "{label} {}",
                i18n::text(
                    ctx.locale,
                    "errors.messages.less_than_or_equal_to",
                    &[("count", i18n::Arg::Count(100))]
                )
            )),
            _ => {}
        }
    }
    (!errors.is_empty()).then(|| {
        crate::web::bot::draft::sentence(
            &errors.iter().map(String::as_str).collect::<Vec<_>>(),
            ctx.locale,
        )
    })
}
fn save(c: &Connection, app: &App, rule: &model::Rule, ctx: &Ctx) -> Result<(), WebError> {
    // ponytail: W2 owns validation and writes across web, REST and MCP, plus durable evaluations.
    let mut settings = rule.settings.clone();
    settings["withdrawal_percentage"] = json!(rule.percentage());
    let settings = settings.to_string();
    let at = ctx.now.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    let old: (String, Option<String>, i64) = c.query_row(
        "SELECT settings,address,status FROM rules WHERE id=?1",
        [rule.id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if old.0 == settings
        && old.2 == rule.status
        && old
            .1
            .as_ref()
            .and_then(|a| app.cipher.decrypt(a).ok())
            .as_deref()
            == Some(&rule.address)
    {
        return Ok(());
    }
    let address = if old
        .1
        .as_ref()
        .and_then(|a| app.cipher.decrypt(a).ok())
        .as_deref()
        == Some(&rule.address)
    {
        old.1
    } else {
        Some(app.cipher.encrypt(&rule.address))
    };
    c.execute("UPDATE rules SET status=?1,settings=?2,address=?3,updated_at=?4,settings_changed_at=CASE WHEN settings<>?2 THEN ?4 ELSE settings_changed_at END WHERE id=?5 AND user_id=?6",
        rusqlite::params![rule.status,settings,address,at,rule.id,rule.user_id])?;
    Ok(())
}

pub async fn tile(
    State(app): State<App>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<i64>,
) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let inner = app.clone();
    let Some(mut rule) = app.db(move |c| model::load(c, &inner, user.id, id)).await? else {
        return Ok(layout::missing());
    };
    let original = rule.clone();
    if ctx.method == axum::http::Method::DELETE {
        // D-RULE-TILE-DELETE-CREATED: a running rule must be stopped before deletion.
        if !matches!(rule.status, 0 | 2) {
            return alert(
                &ctx,
                "Stop the withdrawal rule before deleting it.".into(),
                true,
            );
        }
        rule.status = 3;
        let (inner, v) = (app.clone(), ctx.clone());
        let deleted = app
            .db(move |c| {
                let tx = crate::engine::model::immediate(c)?;
                let Some(mut current) = model::load(&tx, &inner, rule.user_id, rule.id)? else {
                    return Ok(false);
                };
                if !matches!(current.status, 0 | 2) {
                    return Ok(false);
                }
                current.status = 3;
                save(&tx, &inner, &current, &v)?;
                tx.commit()?;
                Ok(true)
            })
            .await?;
        if !deleted {
            return alert(
                &ctx,
                "Stop the withdrawal rule before deleting it.".into(),
                true,
            );
        }
        return Ok(crate::web::wizard::stream(
            StatusCode::OK,
            turbo::refresh().into(),
        ));
    }
    if !ctx
        .params
        .form
        .iter()
        .any(|(k, _)| k.starts_with("rules_withdrawal["))
    {
        return Ok(layout::bad_request());
    }
    let field = |name: &str| ctx.params.form(&format!("rules_withdrawal[{name}]"));
    let status = field("status");
    let changed = matches!(status, Some("scheduled")) && rule.status != 1
        || matches!(status, Some("stopped")) && rule.status != 2;
    for name in [
        "withdrawal_percentage",
        "threshold_type",
        "max_fee_percentage",
        "min_amount",
        "max_interval",
    ] {
        if let Some(value) = field(name) {
            if ["withdrawal_percentage", "max_fee_percentage", "min_amount"].contains(&name)
                && crate::ruby::blank(value)
            {
                continue;
            }
            rule.settings[name] = if name == "max_interval" && crate::ruby::blank(value) {
                serde_json::Value::Null
            } else {
                json!(value)
            };
        }
    }
    let mut destination_version = None;
    let posted = match field("destination")
        .map(reads::Identity::parse)
        .transpose()
    {
        Ok(identity) => identity,
        Err(_) => {
            return alert(
                &ctx,
                i18n::text(ctx.locale, crate::venue::withdrawal::NOT_LISTED_KEY, &[]),
                true,
            )
        }
    };
    if let Some(identity) = &posted {
        let Some(kind) = crate::venue::withdrawal::Kind::of(&rule.exchange.class) else {
            return Ok(layout::bad_request());
        };
        identity.apply(&mut rule.address, &mut rule.settings, kind);
    } else if let Some(address) = field("address").filter(|s| !crate::ruby::blank(s)) {
        // Legacy forms may resubmit the same address with unrelated settings. Keep its key and tag.
        if rule.address != address {
            rule.address = address.into();
            rule.settings["address_name"] = serde_json::Value::Null;
        }
    }
    if posted.is_some() || field("address").is_some_and(|s| !crate::ruby::blank(s)) {
        let list = if rule.status == 2 {
            reads::listing(&app, &rule, true).await?
        } else {
            None
        };
        let destination = list
            .as_ref()
            .filter(|l| {
                posted
                    .as_ref()
                    .is_none_or(|identity| identity.listed(&rule, l))
            })
            .and_then(|l| reads::select(&rule, l).ok());
        let Some(destination) = destination else {
            return alert(
                &ctx,
                i18n::text(ctx.locale, crate::venue::withdrawal::NOT_LISTED_KEY, &[]),
                true,
            );
        };
        destination_version = list.as_ref().map(|l| l.version.clone());
        reads::store(&mut rule.settings, &destination);
    }
    let settings_submitted = [
        "withdrawal_percentage",
        "threshold_type",
        "max_fee_percentage",
        "min_amount",
        "max_interval",
        "address",
        "destination",
    ]
    .iter()
    .any(|name| field(name).is_some());
    if settings_submitted {
        if let Some(error) = validation(&rule, &ctx) {
            return alert(&ctx, error, true);
        }
    }
    if changed {
        rule.status = if status == Some("scheduled") { 1 } else { 2 };
    }
    let start_error = validation(&rule, &ctx);
    if let Some(error) = &start_error {
        if !settings_submitted {
            return alert(&ctx, error.clone(), true);
        }
    }
    // A disabled placeholder contributes no destination to form data. Recheck the stored
    // selection before accepting an unrelated edit or start from a stopped tile.
    let checked_listing = if original.status == 2
        && posted.is_none()
        && field("address").is_none_or(crate::ruby::blank)
    {
        let list = reads::listing(&app, &rule, false).await?;
        if let Some(list) = &list {
            if reads::picker(list, &rule).1.is_none() {
                return alert(
                    &ctx,
                    i18n::text(ctx.locale, crate::venue::withdrawal::NOT_LISTED_KEY, &[]),
                    true,
                );
            }
            destination_version = Some(list.version.clone());
        }
        Some(list)
    } else {
        None
    };
    // Rails saves edits before attempting a start; a rejected start retains those edits.
    // ponytail: W2 decides the atomic shared-service behavior on every surface.
    let mut saved = rule.clone();
    if start_error.is_some() {
        saved.status = original.status;
    }
    let (inner, v) = (app.clone(), ctx.clone());
    let committed = app
        .db(move |c| {
            let tx = crate::engine::model::immediate(c)?;
            if let Some(version) = destination_version {
                if !crate::engine::model::credential_is_current(&tx, &version)? {
                    return Ok(false);
                }
            }
            let Some(current) = model::load(&tx, &inner, saved.user_id, saved.id)? else {
                return Ok(false);
            };
            if current.status != original.status
                || current.settings != original.settings
                || current.address != original.address
            {
                return Ok(false);
            }
            save(&tx, &inner, &saved, &v)?;
            tx.commit()?;
            Ok(true)
        })
        .await?;
    if !committed {
        return alert(
            &ctx,
            i18n::text(ctx.locale, crate::venue::withdrawal::NOT_LISTED_KEY, &[]),
            true,
        );
    }
    if let Some(error) = start_error {
        return alert(&ctx, error, true);
    }
    if changed {
        let kind = if rule.status == 1 {
            "success"
        } else {
            flash::NOTICE
        };
        let text = ctx.t(if rule.status == 1 {
            "rules.tile.activated"
        } else {
            "rules.tile.deactivated"
        });
        let flash = flash::render(&flash::take(&ctx.session, &[(kind, text)]))?;
        Ok(crate::web::wizard::stream(
            StatusCode::OK,
            turbo::prepend_flash(flash.trim_end()) + turbo::refresh(),
        ))
    } else {
        let list = if rule.status == 2 {
            match checked_listing {
                Some(list) => list,
                None => reads::listing(&app, &rule, false).await?,
            }
        } else {
            None
        };
        Ok(crate::web::wizard::stream(
            StatusCode::OK,
            turbo::stream(
                "replace",
                &format!("rules_withdrawal_{id}"),
                &view::tile(&ctx, &rule, list.as_ref())?,
            ),
        ))
    }
}
pub async fn confirm_destroy(
    State(app): State<App>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<i64>,
) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let inner = app.clone();
    let Some(rule) = app.db(move |c| model::load(c, &inner, user.id, id)).await? else {
        return Ok(layout::missing());
    };
    #[derive(Template)]
    #[template(path = "rules/confirm_destroy.html")]
    struct Confirm<'a> {
        v: &'a Ctx,
        title: String,
        action: String,
        csrf: String,
    }
    let body = Confirm {
        v: &ctx,
        title: format!("{} — {}", rule.asset.symbol(), rule.exchange.name),
        action: ctx.path(&format!("{BASE}/{id}")),
        csrf: ctx.csrf_token(),
    }
    .render()?;
    Ok(layout::html(StatusCode::OK, body))
}
/// D-RULE-POST-404: Rails routes the resource POST but defines no create action.
pub async fn missing() -> Response {
    layout::missing()
}
