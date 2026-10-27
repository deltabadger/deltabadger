//! Rails Idempotency: a durable claim, then the action, then its byte-for-byte replayable response.
//! A crash after claiming leaves in_progress; it never silently runs a sale twice.
use super::{rest::{Answer,Input},App,WebError};
use axum::http::StatusCode;
use rusqlite::{Connection,OptionalExtension};
use serde_json::{json,Value};
use sha2::{Digest,Sha256};

/// JSON.generate on the recursively sorted payload. Ruby Float#to_s supplies its exponent spelling.
fn canonical(value:&Value)->String {
    match value {
        Value::Object(map)=>{
            let mut keys=map.keys().collect::<Vec<_>>();keys.sort();
            format!("{{{}}}",keys.into_iter().map(|k|format!("{}:{}",json!(k),canonical(&map[k]))).collect::<Vec<_>>().join(","))
        }
        Value::Array(a)=>format!("[{}]",a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        Value::Number(n) if n.is_f64()=>crate::ruby::float_to_s(n.as_f64().unwrap_or_default()),
        v=>v.to_string(),
    }
}
pub(super) fn run(c:&Connection,app:&App,user:i64,input:&Input<'_>,work:impl FnOnce()->Result<Answer,WebError>)->Result<Answer,WebError> {
    let Some(key)=input.idempotency.filter(|s|!s.trim().is_empty()) else {
        return Ok(super::rest::failure(StatusCode::BAD_REQUEST,"idempotency_key_required","Idempotency-Key header is required."));
    };
    let mut payload=super::rest::object(super::bot::action_params::form(&input.params.query).map_err(|_|WebError::Config("invalid query".into()))?);
    payload.extend(input.body().map_err(|_|WebError::Config("invalid body".into()))?);
    payload.insert("_action".into(),json!("liquidate"));
    payload.insert("_bot_id".into(),json!(input.path.iter().find(|(k,_)|*k=="id").map(|(_,v)|v.as_str()).unwrap_or("")));
    let fingerprint=hex::encode(Sha256::digest(canonical(&Value::Object(payload))));
    let now=crate::codec::format_time(app.now());
    let tx=crate::engine::model::immediate(c)?;
    let existing:Option<(String,String,Option<i64>,Option<String>)>=tx.query_row("SELECT state,request_fingerprint,response_status,response_body FROM idempotency_keys WHERE user_id=?1 AND key=?2",(user,key),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    if let Some((state,previous,status,body))=existing {
        return Ok(if state=="in_progress" {
            super::rest::failure(StatusCode::CONFLICT,"idempotency_in_progress","A request with this Idempotency-Key is already in progress.")
        } else if previous!=fingerprint {
            super::rest::failure(StatusCode::CONFLICT,"idempotency_key_reused","This Idempotency-Key was used with a different request payload.")
        } else {
            Answer::Json(status.and_then(|s|u16::try_from(s).ok()).and_then(|s|StatusCode::from_u16(s).ok()).ok_or_else(||WebError::Config("invalid stored response status".into()))?,
                body.ok_or_else(||WebError::Config("missing stored response".into()))?)
        });
    }
    tx.execute("INSERT INTO idempotency_keys(user_id,key,request_fingerprint,state,locked_at,created_at,updated_at) VALUES (?1,?2,?3,'in_progress',?4,?4,?4)",(user,key,fingerprint,&now))?;
    let id=tx.last_insert_rowid();tx.commit()?;
    let answer=work()?;
    let Answer::Json(status,body)=answer else {return Ok(answer)};
    // Idempotency uses JSON.generate, not the HTML-escaping render-json encoder.
    let value:Value=serde_json::from_str(&body).map_err(|e|WebError::Config(format!("invalid response: {e}")))?;
    let body=value.to_string();
    c.execute("UPDATE idempotency_keys SET state='completed',response_status=?1,response_body=?2,updated_at=?3 WHERE id=?4",(i64::from(status.as_u16()),&body,crate::codec::format_time(app.now()),id))?;
    Ok(Answer::Json(status,body))
}
