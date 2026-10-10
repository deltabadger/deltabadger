use super::clients;
use crate::{
    codec::format_time,
    web::{
        auth,
        layout::{Ctx, Page},
        oauth::{self, TOOL_GROUPS},
        shell::{self, Shell},
        App, WebError,
    },
};
use askama::Template;
use axum::{http::StatusCode, response::Response};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::Value;
#[derive(Template)]
#[template(path = "settings/api.html")]
struct Api<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    root: String,
    connected: String,
    tools: String,
    mcp_widget: String,
}
#[derive(Template)]
#[template(path = "settings/tool_permissions.html")]
struct Tools<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    mcp: Vec<String>,
    rest: Vec<String>,
}
#[derive(Template)]
#[template(path = "settings/mcp.html")]
struct Mcp<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    dry_run: bool,
    root: String,
}
impl Tools<'_> {
    fn on(&self, surface: &str, tool: &str) -> bool {
        if surface == "mcp" {
            self.mcp.iter().any(|s| s == tool)
        } else {
            self.rest.iter().any(|s| s == tool)
        }
    }
    fn group_state(&self, surface: &str, group: &str) -> &'static str {
        let tools = TOOL_GROUPS
            .iter()
            .find(|(name, _)| *name == group)
            .map(|(_, tools)| *tools)
            .unwrap_or(&[]);
        let n = tools.iter().filter(|tool| self.on(surface, tool)).count();
        if n == 0 {
            "off"
        } else if n == tools.len() {
            "on"
        } else {
            "partial"
        }
    }
}
fn ensure_personal(c: &Connection, user: i64, now: &str) -> Result<(), WebError> {
    let id:Option<i64>=c.query_row("SELECT id FROM oauth_applications WHERE personal_owner_id=?1 AND personal_access_token=1",[user],|r|r.get(0)).optional()?;
    let id = match id {
        Some(id) => id,
        None => {
            c.execute("INSERT INTO oauth_applications(name,uid,secret,redirect_uri,scopes,confidential,personal_access_token,personal_owner_id,created_at,updated_at)VALUES('Personal API token',?1,?2,'https://localhost/personal-access-token','api',0,1,?3,?4,?4)",(oauth::new_token(),oauth::new_token(),user,now))?;
            c.last_insert_rowid()
        }
    };
    let active:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM oauth_access_tokens WHERE application_id=?1 AND resource_owner_id=?2 AND revoked_at IS NULL AND (expires_in IS NULL OR datetime(created_at,'+'||expires_in||' seconds')>datetime(?3)))",(id,user,now),|r|r.get(0))?;
    if !active {
        let token = rand::random::<[u8; 32]>()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,expires_in,created_at)VALUES(?1,?2,?3,'api',NULL,?4)",(id,user,token,now))?;
    }
    Ok(())
}
pub async fn show(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let inner = app.clone();
    let v = ctx.clone();
    let csrf = ctx.csrf_token();
    let token = csrf.clone();
    let (mcp, rest, dry_run, connected, shell) = app
        .db(move |c| {
            let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
            ensure_personal(&tx, user.id, &format_time(inner.now()))?;
            let settings: Option<String> = tx.query_row(
                "SELECT mcp_settings FROM users WHERE id=?1",
                [user.id],
                |r| r.get(0),
            )?;
            let settings: Value = settings
                .map(|s| serde_json::from_str(&s))
                .transpose()
                .map_err(|_| WebError::Config("invalid MCP settings".into()))?
                .unwrap_or(Value::Null);
            let output = (
                clients::enabled(&tx, user.id, "mcp")?,
                clients::enabled(&tx, user.id, "rest")?,
                settings["dry_run"] == true,
                clients::widget(&tx, &v, &token, user.id)?,
                Shell::load(&tx, &inner, &user)?,
            );
            tx.commit()?;
            Ok(output)
        })
        .await?;
    let user = ctx
        .user()
        .ok_or_else(|| WebError::Config("missing API owner".into()))?;
    let root = app
        .config
        .own_origin
        .clone()
        .unwrap_or_else(|| "http://localhost:3000".into());
    shell::application(
        &ctx,
        &csrf,
        user,
        &shell,
        Page {
            status: StatusCode::OK,
            body: Api {
                v: &ctx,
                csrf: &csrf,
                tools: Tools {
                    v: &ctx,
                    csrf: &csrf,
                    mcp,
                    rest,
                }
                .render()?,
                mcp_widget: Mcp {
                    v: &ctx,
                    csrf: &csrf,
                    dry_run,
                    root: root.clone(),
                }
                .render()?,
                root,
                connected,
            }
            .render()?,
            flash_now: vec![],
        },
    )
}

pub async fn write(app: App, ctx: Ctx) -> Result<Response, WebError> {
    use crate::web::{oauth::TOOL_DEFAULTS, turbo};
    use axum::{http::header, response::IntoResponse};
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let action = ctx.params.route_path.rsplit('/').next().unwrap_or("");
    let dry_run = action == "update_mcp_dry_run";
    let group = action.contains("group");
    let surface = if action.contains("rest") {
        "rest"
    } else {
        "mcp"
    };
    let requested = ctx
        .params
        .form(if group { "group" } else { "tool_name" })
        .unwrap_or("")
        .to_string();
    let names: Vec<&'static str> = if dry_run {
        vec![]
    } else if group {
        TOOL_GROUPS
            .iter()
            .find(|(name, _)| *name == requested)
            .map(|(_, tools)| tools.to_vec())
            .unwrap_or_default()
    } else {
        TOOL_DEFAULTS
            .iter()
            .find(|(name, _)| *name == requested)
            .map(|(name, _)| vec![*name])
            .unwrap_or_default()
    };
    if !dry_run && names.is_empty() {
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
    let body = app
        .db(move |c| {
            let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
            let column = if surface == "mcp" {
                "mcp_settings"
            } else {
                "rest_settings"
            };
            let previous: Option<String> = tx.query_row(
                &format!("SELECT {column} FROM users WHERE id=?1"),
                [user.id],
                |r| r.get(0),
            )?;
            let mut settings: Value = previous
                .as_ref()
                .map(|s| serde_json::from_str(s))
                .transpose()
                .map_err(|_| WebError::Config("invalid tool settings".into()))?
                .unwrap_or_else(|| serde_json::json!({}));
            let object = settings
                .as_object_mut()
                .ok_or_else(|| WebError::Config("tool settings is not an object".into()))?;
            if dry_run {
                object.insert("dry_run".into(), on.into());
            } else {
                let overrides = object
                    .entry("tool_permissions")
                    .or_insert_with(|| serde_json::json!({}))
                    .as_object_mut()
                    .ok_or_else(|| WebError::Config("tool overrides is not an object".into()))?;
                for name in names {
                    overrides.insert(name.into(), on.into());
                }
            }
            let same = previous
                .map(|s| serde_json::from_str::<Value>(&s))
                .transpose()
                .map_err(|_| WebError::Config("invalid old tool settings".into()))?
                == Some(settings.clone());
            if !same {
                tx.execute(
                    &format!("UPDATE users SET {column}=?1,updated_at=?2 WHERE id=?3"),
                    (settings.to_string(), format_time(inner.now()), user.id),
                )?;
            }
            let body = if dry_run {
                let root = inner
                    .config
                    .own_origin
                    .clone()
                    .unwrap_or_else(|| "http://localhost:3000".into());
                turbo::stream(
                    "replace",
                    "mcp_settings",
                    Mcp {
                        v: &v,
                        csrf: &token,
                        dry_run: on,
                        root,
                    }
                    .render()?
                    .trim_end(),
                )
            } else {
                turbo::stream(
                    "replace",
                    "tool_permissions",
                    Tools {
                        v: &v,
                        csrf: &token,
                        mcp: clients::enabled(&tx, user.id, "mcp")?,
                        rest: clients::enabled(&tx, user.id, "rest")?,
                    }
                    .render()?
                    .trim_end(),
                ) + &turbo::stream(
                    "replace",
                    "connected_clients",
                    clients::widget(&tx, &v, &token, user.id)?.trim_end(),
                )
            };
            tx.commit()?;
            Ok(body)
        })
        .await?;
    Ok((
        [(header::CONTENT_TYPE, crate::web::turbo::CONTENT_TYPE)],
        body,
    )
        .into_response())
}
