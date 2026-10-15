use super::clients;
use crate::{
    codec::format_time,
    web::{
        auth,
        layout::Ctx,
        oauth::{self, TOOL_GROUPS},
        App, WebError,
    },
};
use axum::{http::StatusCode, response::Response};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::Value;
/// User#personal_api_application, created when missing (create_personal_api_app_safely!). Doorkeeper generates a uid,
/// and no secret for an application that is not confidential (the column is nullable).
fn personal_app(c: &Connection, user: i64, now: &str) -> Result<i64, WebError> {
    let id:Option<i64>=c.query_row("SELECT id FROM oauth_applications WHERE personal_owner_id=?1 AND personal_access_token=1",[user],|r|r.get(0)).optional()?;
    Ok(match id {
        Some(id) => id,
        None => {
            c.execute("INSERT INTO oauth_applications(name,uid,secret,redirect_uri,scopes,confidential,personal_access_token,personal_owner_id,created_at,updated_at)VALUES('Personal API token',?1,NULL,'https://localhost/personal-access-token','api',0,1,?2,?3,?3)",(oauth::new_token(),user,now))?;
            c.last_insert_rowid()
        }
    })
}
/// User#mint_personal_token!, scoped to api, with no expiry. The token is Doorkeeper's own: its create callback replaces
/// the `SecureRandom.hex(32)` the model passes with a fresh `SecureRandom.urlsafe_base64(32)`.
fn mint(c: &Connection, app: i64, user: i64, now: &str) -> Result<String, WebError> {
    let token = oauth::new_token();
    c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,expires_in,created_at)VALUES(?1,?2,?3,'api',NULL,?4)",(app,user,&token,now))?;
    Ok(token)
}
pub(super) fn ensure_personal(c: &Connection, user: i64, now: &str) -> Result<(), WebError> {
    let id = personal_app(c, user, now)?;
    let active:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM oauth_access_tokens WHERE application_id=?1 AND resource_owner_id=?2 AND revoked_at IS NULL AND (expires_in IS NULL OR datetime(created_at,'+'||expires_in||' seconds')>datetime(?3)))",(id,user,now),|r|r.get(0))?;
    if !active {
        mint(c, id, user, now)?;
    }
    Ok(())
}
/// POST /settings/regenerate_api_token (User#regenerate_personal_api_token!): every live personal token is revoked and
/// a new one minted; the answer is the one response that shows it.
pub async fn regenerate(app: App, ctx: Ctx) -> Result<Response, WebError> {
    use axum::{http::header, response::IntoResponse};
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let (inner, v, csrf) = (app.clone(), ctx.clone(), ctx.csrf_token());
    let widget = app
        .db(move |c| {
            let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
            let now = format_time(inner.now());
            let id = personal_app(&tx, user.id, &now)?;
            tx.execute(
                "UPDATE oauth_access_tokens SET revoked_at=?1 WHERE application_id=?2 AND resource_owner_id=?3 AND revoked_at IS NULL",
                (&now, id, user.id),
            )?;
            let token = mint(&tx, id, user.id, &now)?;
            let widget = super::view::rest_widget(&inner, &v, &csrf, &token);
            tx.commit()?;
            Ok(widget)
        })
        .await?;
    Ok((
        [(header::CONTENT_TYPE, crate::web::turbo::CONTENT_TYPE)],
        crate::web::turbo::stream("replace", "rest_settings", widget.trim_end()),
    )
        .into_response())
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
                turbo::stream("replace", "mcp_settings", super::view::mcp_widget(&inner, &v, &token, on).trim_end())
            } else {
                // A user-level switch changes what every client may hold, so the client cards re-render with the matrix.
                turbo::stream("replace", "tool_permissions", super::view::tools_widget(&v, &token, surface, &clients::enabled(&tx, user.id, surface)?).trim_end())
                    + &turbo::stream("replace", "connected_clients", clients::widget(&tx, &v, &token, user.id, surface)?.trim_end())
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
