//! A1: request/response formatting only. W2 owns every rule decision and mutation.
use super::service::{self, Command, ListedRule, Patch, Status};
use crate::web::{
    bot_api::{Domain, Read, Res},
    App, WebError,
};
use serde_json::{json, Value};

// Rails' update response order, followed by the additional W2 matrix fields.
const FIELDS: [&str; 9] = [
    "withdrawal_percentage",
    "max_fee_percentage",
    "min_amount",
    "threshold_type",
    "max_interval",
    "address",
    "network",
    "address_tag",
    "address_name",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    List,
    Create,
    Update,
    Start,
    Stop,
    Delete,
}
impl Action {
    pub fn tool(name: &str) -> Option<Self> {
        Some(match name {
            "list_rules" => Self::List,
            "create_rule" => Self::Create,
            "update_rule_settings" => Self::Update,
            "start_rule" => Self::Start,
            "stop_rule" => Self::Stop,
            "delete_rule" => Self::Delete,
            _ => return None,
        })
    }
}
pub struct Request {
    pub user: i64,
    pub action: Action,
    pub args: Value,
}
impl Request {
    /// ActionMCP's number properties are Floats. Preserve their Rails string representation.
    /// max_interval is an integer (days), and identifiers are checked before truncation.
    pub fn mcp(user: i64, action: Action, mut args: Value) -> Self {
        for field in ["withdrawal_percentage", "max_fee_percentage", "min_amount"] {
            if let Some(n) = args[field].as_f64() {
                args[field] = json!(crate::figures::num::float_to_s(n));
            }
        }
        if let Some(n) = args["max_interval"]
            .as_f64()
            .filter(|n| n.fract() == 0.0 && (i64::MIN as f64..-(i64::MIN as f64)).contains(n))
        {
            args["max_interval"] = json!(n as i64);
        }
        if let Some(n) = args["rule_id"].as_f64() {
            args["id"] = json!(if (i64::MIN as f64..-(i64::MIN as f64)).contains(&n) {
                n as i64
            } else {
                0
            });
        }
        Self { user, action, args }
    }
    pub async fn perform(self, app: &App) -> Result<Read, WebError> {
        let Self { user, action, args } = self;
        if action == Action::List {
            let inner = app.clone();
            return app
                .db(move |c| {
                    let rows: Vec<_> = service::list(c, &inner, user)?
                        .into_iter()
                        .map(row)
                        .collect();
                    let text = list_text(&rows);
                    Ok(success(json!({"count":rows.len(),"rules":rows}), text))
                })
                .await;
        }
        let text = |name: &str| crate::web::mcp::tools::str_value(&args[name]);
        let patch = || Patch {
            settings: Value::Object(
                FIELDS
                    .iter()
                    .filter(|k| **k != "address")
                    .filter_map(|k| {
                        args.get(*k)
                            .filter(|v| !v.is_array() && !v.is_object())
                            .map(|v| ((*k).into(), v.clone()))
                    })
                    .collect(),
            ),
            address: args
                .get("address")
                .filter(|v| !v.is_array() && !v.is_object())
                .map(crate::web::mcp::tools::str_value)
                .filter(|v| action == Action::Create || !crate::ruby::blank(v)),
            ..Default::default()
        };
        let id = args
            .get("id")
            .and_then(crate::web::bot_api::to_i)
            .unwrap_or(0);
        let result = match action {
            Action::Create => {
                service::create_named(app, user, &text("exchange_name"), &text("asset"), patch())
                    .await?
            }
            Action::Update => service::call(app, user, Command::Update(id, patch())).await?,
            Action::Start => service::call(app, user, Command::Start(id, Patch::default())).await?,
            Action::Stop => service::call(app, user, Command::Stop(id)).await?,
            Action::Delete => service::call(app, user, Command::Delete(id)).await?,
            Action::List => unreachable!(),
        };
        let out = match result {
            Ok(out) => out,
            Err(e) => {
                let status = match e.status {
                    Status::NotFound => Domain::NotFound,
                    Status::ValidationFailed => Domain::ValidationFailed,
                    Status::PermissionDenied => Domain::PermissionDenied,
                    Status::Conflict => Domain::Conflict,
                };
                return Ok(Read {
                    res: Res::failure(status, e.code, e.reason.clone()),
                    text: e.reason,
                    is_error: true,
                });
            }
        };
        let id = out.id;
        Ok(match action {
            Action::Create => {
                let inner = app.clone();
                let data = app
                    .db(move |c| {
                        service::list(c, &inner, user)?
                            .into_iter()
                            .find(|r| r.id == id)
                            .map(row)
                            .ok_or_else(|| {
                                WebError::Config("created rule is no longer visible".into())
                            })
                    })
                    .await?;
                let s = |k: &str| crate::web::mcp::tools::str_value(&data[k]);
                let text = format!("Rule #{id} created (stopped): {}% of {} on {} to {}. Use 'start_rule' to activate.", s("withdrawal_percentage"), s("asset"), s("exchange"), s("address"));
                success(data, text)
            }
            Action::Update => {
                let updated: Vec<_> = FIELDS
                    .iter()
                    .copied()
                    .filter(|k| {
                        args.get(*k).is_some_and(|v| {
                            !v.is_array()
                                && !v.is_object()
                                && (*k == "max_interval"
                                    || !crate::ruby::blank(&crate::web::mcp::tools::str_value(v)))
                        })
                    })
                    .collect();
                success(
                    json!({"id":id,"updated":updated}),
                    format!("Rule #{id} settings updated: {}.", updated.join(", ")),
                )
            }
            Action::Start | Action::Stop | Action::Delete => {
                let verb = match action {
                    Action::Start => "started",
                    Action::Stop => "stopped",
                    _ => "deleted",
                };
                success(
                    json!({"id":id,"status":out.status}),
                    format!("Rule #{id} {verb}."),
                )
            }
            Action::List => unreachable!(),
        })
    }
}
fn success(data: Value, text: String) -> Read {
    Read {
        res: Res::success(data),
        text,
        is_error: false,
    }
}
fn row(r: ListedRule) -> Value {
    let unresolved = r.unresolved_withdrawal.map(
        |u| json!({"state":u.state,"reason":u.reason,"since":u.since,"candidates":u.candidates}),
    );
    json!({"id":r.id,"type":r.rule_type,"status":r.status,"exchange":r.exchange,"asset":r.asset,
        "address":r.address,"address_tag":r.address_tag,"network":r.network,"threshold_type":r.threshold_type,
        "withdrawal_percentage":r.withdrawal_percentage,"max_fee_percentage":r.max_fee_percentage,"min_amount":r.min_amount,
        "max_interval":r.max_interval,"unresolved_withdrawal":unresolved})
}
fn list_text(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "No rules found.".into();
    }
    let lines: Vec<_> = rows
        .iter()
        .map(|r| {
            let s = |k: &str| crate::web::mcp::tools::str_value(&r[k]);
            let threshold = if r["threshold_type"] == "min_amount" {
                format!("min {}", s("min_amount"))
            } else {
                format!("max fee {}%", s("max_fee_percentage"))
            };
            let mut line = format!(
                "- #{} | {} on {} | {} | {}% to {} | {threshold}",
                s("id"),
                s("asset"),
                s("exchange"),
                s("status"),
                s("withdrawal_percentage"),
                s("address")
            );
            if !r["max_interval"].is_null() {
                line += &format!(" | max interval {} days", s("max_interval"));
            }
            if !r["unresolved_withdrawal"].is_null() {
                line += &format!(
                    " | unresolved_withdrawal: {}",
                    crate::web::rest::encode(&r["unresolved_withdrawal"])
                );
            }
            line
        })
        .collect();
    format!("Rules ({}):\n{}", rows.len(), lines.join("\n"))
}
