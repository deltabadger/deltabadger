//! Owner-scoped clients share the OAuth catalogue; revocation preserves other users' live credentials.
use crate::{
    codec::{format_time, parse_time},
    web::{
        auth, flash,
        layout::{self, Ctx, Page},
        oauth::{TOOL_DEFAULTS, TOOL_GROUPS},
        shell::{self, Shell},
        turbo, App, WebError,
    },
};
use askama::Template;
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::Value;
const LIVE:&str="SELECT a.id FROM oauth_applications a WHERE COALESCE(a.personal_access_token,0)=0 AND (EXISTS(SELECT 1 FROM oauth_access_tokens t WHERE t.application_id=a.id AND t.resource_owner_id=?1 AND t.revoked_at IS NULL) OR EXISTS(SELECT 1 FROM oauth_access_grants g WHERE g.application_id=a.id AND g.resource_owner_id=?1 AND g.revoked_at IS NULL AND datetime(g.created_at,'+'||g.expires_in||' seconds')>datetime(?2)))";
#[derive(Clone)]
pub struct Client {
    pub id: i64,
    pub name: String,
    pub date: String,
    pub groups: Vec<Group>,
}
#[derive(Clone)]
pub struct Group {
    pub name: &'static str,
    pub label: String,
    pub switches: Vec<Switch>,
}
#[derive(Clone)]
pub struct Switch {
    pub surface: &'static str,
    pub present: bool,
    pub state: &'static str,
    pub enabled: &'static str,
}
pub fn enabled(c: &Connection, user: i64, surface: &str) -> Result<Vec<String>, WebError> {
    let column = if surface == "mcp" {
        "mcp_settings"
    } else {
        "rest_settings"
    };
    let text: Option<String> = c.query_row(
        &format!("SELECT {column} FROM users WHERE id=?1"),
        [user],
        |r| r.get(0),
    )?;
    let settings: Value = text
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|_| WebError::Config("invalid tool settings".into()))?
        .unwrap_or(Value::Null);
    let on = |v: &Value| !matches!(v, Value::Null | Value::Bool(false));
    let overrides = settings["tool_permissions"].as_object();
    let mut enabled: Vec<String> = TOOL_DEFAULTS
        .iter()
        .filter(|(name, default)| {
            overrides
                .and_then(|o| o.get(*name))
                .map_or(surface == "mcp" && *default, on)
        })
        .map(|(name, _)| name.to_string())
        .collect();
    enabled.extend(
        overrides
            .into_iter()
            .flatten()
            .filter(|(name, v)| {
                on(v) && !TOOL_DEFAULTS.iter().any(|(tool, _)| *tool == name.as_str())
            })
            .map(|(name, _)| name.clone()),
    );
    Ok(enabled)
}
fn grants(
    c: &Connection,
    user: i64,
    application: i64,
    surface: &str,
) -> Result<Vec<String>, WebError> {
    let column = if surface == "mcp" {
        "mcp_tools"
    } else {
        "rest_tools"
    };
    let text:Option<String>=c.query_row(&format!("SELECT {column} FROM connected_clients WHERE user_id=?1 AND oauth_application_id=?2"),(user,application),|r|r.get(0)).optional()?;
    let list: Value = text
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|_| WebError::Config("invalid client grants".into()))?
        .unwrap_or(Value::Null);
    let mut names = vec![];
    for name in list.as_array().into_iter().flatten() {
        let name = name
            .as_str()
            .map_or_else(|| name.to_string(), str::to_string);
        if TOOL_DEFAULTS.iter().any(|(tool, _)| *tool == name) && !names.contains(&name) {
            names.push(name);
        }
    }
    Ok(names)
}
fn connected(c: &Connection, ctx: &Ctx, user: i64, id: i64) -> Result<bool, WebError> {
    Ok(c.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM ({LIVE}) WHERE id=?3)"),
        (user, format_time(ctx.now), id),
        |r| r.get(0),
    )?)
}
fn load(c: &Connection, ctx: &Ctx, user: i64) -> Result<Vec<Client>, WebError> {
    let mut statement=c.prepare(&format!("SELECT id,name,created_at,scopes FROM oauth_applications WHERE id IN ({LIVE}) ORDER BY created_at DESC"))?;
    let rows = statement
        .query_map((user, format_time(ctx.now)), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut clients = vec![];
    for (id, name, created, scopes) in rows {
        let mut groups = vec![];
        for (group, tools) in TOOL_GROUPS {
            let mut switches = vec![];
            for surface in ["mcp", "rest"] {
                let granted = grants(c, user, id, surface)?;
                let available = enabled(c, user, surface)?;
                let here = granted
                    .iter()
                    .filter(|name| tools.contains(&name.as_str()))
                    .collect::<Vec<_>>();
                let state = if here.is_empty() {
                    "off"
                } else if available
                    .iter()
                    .filter(|name| tools.contains(&name.as_str()))
                    .all(|name| here.contains(&name))
                {
                    "on"
                } else {
                    "partial"
                };
                switches.push(Switch {
                    surface,
                    present: scopes
                        .split_whitespace()
                        .any(|scope| scope == if surface == "mcp" { "mcp" } else { "api" }),
                    state,
                    enabled: if here.is_empty() { "1" } else { "0" },
                });
            }
            groups.push(Group {
                name: group,
                label: ctx.t(&format!("settings.mcp.tools_{group}")),
                switches,
            });
        }
        let date = parse_time(&created)
            .map_err(|_| WebError::Config("invalid client creation date".into()))?;
        // Rails I18n's time.formats.short; the locale formatter is pinned in the page grid.
        clients.push(Client {
            id,
            name,
            date: date.format("%d %b %H:%M").to_string(),
            groups,
        });
    }
    Ok(clients)
}
#[derive(Template)]
#[template(path = "settings/connected_clients.html")]
struct Widget<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    clients: Vec<Client>,
}
pub fn widget(c: &Connection, ctx: &Ctx, csrf: &str, user: i64) -> Result<String, WebError> {
    Ok(Widget {
        v: ctx,
        csrf,
        clients: load(c, ctx, user)?,
    }
    .render()?)
}
#[derive(Template)]
#[template(path = "settings/revoke_client.html")]
struct Revoke<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    id: i64,
    name: String,
}
pub async fn show(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let id = ctx
        .params
        .route_path
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<i64>().ok());
    let Some(id) = id else {
        return Ok(layout::missing());
    };
    let inner = app.clone();
    let v = ctx.clone();
    let found = app
        .db(move |c| {
            if !connected(c, &v, user.id, id)? {
                return Ok(None);
            }
            Ok(Some((
                c.query_row(
                    "SELECT name FROM oauth_applications WHERE id=?1",
                    [id],
                    |r| r.get::<_, String>(0),
                )?,
                Shell::load(c, &inner, &user)?,
            )))
        })
        .await?;
    let Some((name, shell)) = found else {
        return Ok(layout::missing());
    };
    let csrf = ctx.csrf_token();
    shell::application(
        &ctx,
        &csrf,
        ctx.user()
            .ok_or_else(|| WebError::Config("missing client owner".into()))?,
        &shell,
        Page {
            status: StatusCode::OK,
            body: Revoke {
                v: &ctx,
                csrf: &csrf,
                id,
                name,
            }
            .render()?,
            flash_now: vec![],
        },
    )
}
pub async fn write(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let revoke = ctx
        .params
        .route_path
        .starts_with("/settings/revoke_mcp_client/");
    let id = ctx
        .params
        .route_path
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<i64>().ok());
    let Some(id) = id else {
        return Ok(layout::missing());
    };
    let surface = ctx.params.form("surface").unwrap_or("").to_string();
    let group = ctx.params.form("group").unwrap_or("").to_string();
    if !revoke
        && (!["mcp", "rest"].contains(&surface.as_str())
            || !TOOL_GROUPS.iter().any(|(name, _)| *name == group))
    {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/vnd.turbo-stream.html",
            )],
        )
            .into_response());
    }
    let on = ctx.params.form("enabled") == Some("1");
    let inner = app.clone();
    let v = ctx.clone();
    let csrf = ctx.csrf_token();
    let token = csrf.clone();
    let body=app.db(move|c|{
  let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;if !connected(&tx,&v,user.id,id)?{return Ok(None);}
  let now=format_time(inner.now());
  if revoke{
   tx.execute("UPDATE oauth_access_tokens SET revoked_at=?1 WHERE application_id=?2 AND resource_owner_id=?3 AND revoked_at IS NULL",(&now,id,user.id))?;
   tx.execute("UPDATE oauth_access_grants SET revoked_at=?1 WHERE application_id=?2 AND resource_owner_id=?3 AND revoked_at IS NULL",(&now,id,user.id))?;
   tx.execute("DELETE FROM connected_clients WHERE user_id=?1 AND oauth_application_id=?2",(user.id,id))?;
   let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM oauth_access_tokens WHERE application_id=?1 AND revoked_at IS NULL) OR EXISTS(SELECT 1 FROM oauth_access_grants WHERE application_id=?1 AND revoked_at IS NULL AND datetime(created_at,'+'||expires_in||' seconds')>datetime(?2))",(id,&now),|r|r.get(0))?;
   if !live{for table in ["connected_clients","oauth_access_tokens","oauth_access_grants"]{let column=if table=="connected_clients"{"oauth_application_id"}else{"application_id"};tx.execute(&format!("DELETE FROM {table} WHERE {column}=?1"),[id])?;}tx.execute("DELETE FROM oauth_applications WHERE id=?1",[id])?;}
  }else{
   let tools=TOOL_GROUPS.iter().find(|(name,_)|*name==group).map(|(_,tools)|*tools).ok_or_else(||WebError::Config("unknown tool group".into()))?;
   let column=if surface=="mcp"{"mcp_tools"}else{"rest_tools"};
   let current=grants(&tx,user.id,id,&surface)?;let mut next=current.clone();
   if on{for name in enabled(&tx,user.id,&surface)?.into_iter().filter(|name|tools.contains(&name.as_str())){if !next.contains(&name){next.push(name);}}}else{next.retain(|name|!tools.contains(&name.as_str()));}
   let found:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM connected_clients WHERE user_id=?1 AND oauth_application_id=?2)",(user.id,id),|r|r.get(0))?;
   if !found{tx.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at)VALUES(?1,?2,'[]','[]',?3,?3)",(user.id,id,&now))?;}
   if next!=current{tx.execute(&format!("UPDATE connected_clients SET {column}=?1,updated_at=?2 WHERE user_id=?3 AND oauth_application_id=?4"),(serde_json::to_string(&next).map_err(|_|WebError::Config("cannot encode client grants".into()))?,&now,user.id,id))?;}
  }
  let body=widget(&tx,&v,&token,user.id)?;tx.commit()?;Ok(Some(body))
 }).await?;
    let Some(body) = body else {
        return Ok(layout::missing());
    };
    let mut stream = turbo::stream("replace", "connected_clients", body.trim_end());
    if revoke {
        stream += &turbo::prepend_flash(
            flash::render(&flash::take(
                &ctx.session,
                &[(flash::NOTICE, ctx.t("settings.mcp.client_revoked"))],
            ))?
            .trim_end(),
        );
    }
    Ok(([(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], stream).into_response())
}
