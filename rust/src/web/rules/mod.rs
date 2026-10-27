//! Withdrawal pages. Rails' controllers and views are pinned by tests/pages.rs.
pub mod model;
mod reads;
pub mod service;
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
        let rules = service::visible_rules(c,&inner,user.id)?;
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

pub async fn tile(
    State(app): State<App>,
    Extension(ctx): Extension<Ctx>,
    Path(id): Path<i64>,
) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)); };
    let field = |name: &str| ctx.params.form(&format!("rules_withdrawal[{name}]"));
    let command = if ctx.method == axum::http::Method::DELETE {
        service::Command::Delete(id)
    } else {
        if !ctx.params.form.iter().any(|(k,_)| k.starts_with("rules_withdrawal[")) {
            return Ok(layout::bad_request());
        }
        let mut patch=service::Patch::settings(json!({}));
        for name in ["withdrawal_percentage","threshold_type","max_fee_percentage","min_amount","max_interval"] {
            if let Some(value)=field(name) { patch.settings[name]=json!(value); }
        }
        patch.address=field("address").filter(|s|!crate::ruby::blank(s)).map(str::to_owned);
        patch.destination=match field("destination").map(reads::Identity::parse).transpose() {
            Ok(identity)=>identity,
            Err(_)=>return alert(&ctx,i18n::text(ctx.locale,crate::venue::withdrawal::NOT_LISTED_KEY,&[]),true),
        };
        let inner=app.clone();
        let Some(rule)=app.db(move|c|model::load(c,&inner,user.id,id)).await? else {return Ok(layout::missing())};
        // Rails has no separate toggle-submit parameter. Created and stopped tiles
        // both post the hidden stopped value when their settings are edited.
        match field("status") {
            Some("scheduled") if rule.status!=1=>service::Command::Start(id,patch),
            Some("stopped") if matches!(rule.status,1|4|5|6)=>service::Command::Stop(id),
            Some("scheduled"|"stopped")|None=>service::Command::Update(id,patch),
            Some(_)=>return Ok(layout::bad_request()),
        }
    };
    let action = match command { service::Command::Start(..)=>1, service::Command::Stop(_)=>2,service::Command::Delete(_)=>3,_=>0 };
    match service::call(&app,user.id,command).await? {
        Err(e) if e.status==service::Status::NotFound=>Ok(layout::missing()),
        Err(e)=>alert(&ctx,if e.code=="address_not_listed" {
            i18n::text(ctx.locale,crate::venue::withdrawal::NOT_LISTED_KEY,&[])
        } else { e.reason },true),
        Ok(_) if action==3=>Ok(crate::web::wizard::stream(StatusCode::OK,turbo::refresh().into())),
        Ok(_) if action!=0=>{
            let kind=if action==1 {"success"} else {flash::NOTICE};
            let text=ctx.t(if action==1 {"rules.tile.activated"} else {"rules.tile.deactivated"});
            let flash=flash::render(&flash::take(&ctx.session,&[(kind,text)]))?;
            Ok(crate::web::wizard::stream(StatusCode::OK,turbo::prepend_flash(flash.trim_end())+turbo::refresh()))
        },
        Ok(_)=>{
            let inner=app.clone();
            let Some(rule)=app.db(move|c|model::load(c,&inner,user.id,id)).await? else {return Ok(layout::missing())};
            let list=if rule.status==2 {reads::listing(&app,&rule,false).await?} else {None};
            Ok(crate::web::wizard::stream(StatusCode::OK,turbo::stream("replace",&format!("rules_withdrawal_{id}"),&view::tile(&ctx,&rule,list.as_ref())?)))
        }
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
