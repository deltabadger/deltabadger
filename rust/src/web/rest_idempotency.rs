//! Rails Idempotency: a durable claim, then the action, then its byte-for-byte replayable response.
//! A crash after claiming leaves in_progress; it never silently runs a sale twice.
//! The claim and the completion are separate steps (`claim`, `complete`), so an action may run between them in
//! another job; `run` is the two around one closure.
use super::{rest::{Answer,Input},App,WebError};
use axum::http::StatusCode;
use rusqlite::{Connection,OptionalExtension};
use serde_json::{json,Map,Value};
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

/// What a claim found: the key is now this request's (`Claimed`, the row id), or the answer to give instead.
pub(super) enum Claim { Claimed(i64), Answer(Answer) }

/// #idempotent_request up to the block: the key is required, then claimed in_progress for this payload's fingerprint, or
/// the stored answer (or a conflict) is the answer.
pub(super) fn claim(c:&Connection,app:&App,user:i64,key:Option<&str>,payload:Map<String,Value>)->Result<Claim,WebError> {
    let Some(key)=key.filter(|s|!s.trim().is_empty()) else {
        return Ok(Claim::Answer(super::rest::failure(StatusCode::BAD_REQUEST,"idempotency_key_required","Idempotency-Key header is required.")));
    };
    let fingerprint=hex::encode(Sha256::digest(canonical(&Value::Object(payload))));
    let now=crate::codec::format_time(app.now());
    let tx=crate::engine::model::immediate(c)?;
    let existing:Option<(String,String,Option<i64>,Option<String>)>=tx.query_row("SELECT state,request_fingerprint,response_status,response_body FROM idempotency_keys WHERE user_id=?1 AND key=?2",(user,key),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    if let Some((state,previous,status,body))=existing {
        return Ok(Claim::Answer(if state=="in_progress" {
            super::rest::failure(StatusCode::CONFLICT,"idempotency_in_progress","A request with this Idempotency-Key is already in progress.")
        } else if previous!=fingerprint {
            super::rest::failure(StatusCode::CONFLICT,"idempotency_key_reused","This Idempotency-Key was used with a different request payload.")
        } else {
            Answer::Json(status.and_then(|s|u16::try_from(s).ok()).and_then(|s|StatusCode::from_u16(s).ok()).ok_or_else(||WebError::Config("invalid stored response status".into()))?,
                body.ok_or_else(||WebError::Config("missing stored response".into()))?)
        }));
    }
    tx.execute("INSERT INTO idempotency_keys(user_id,key,request_fingerprint,state,locked_at,created_at,updated_at) VALUES (?1,?2,?3,'in_progress',?4,?4,?4)",(user,key,fingerprint,&now))?;
    let id=tx.last_insert_rowid();tx.commit()?;
    Ok(Claim::Claimed(id))
}

/// The block's answer stored on the claimed key (state completed) and given back as stored.
pub(super) fn complete(c:&Connection,app:&App,id:i64,answer:Answer)->Result<Answer,WebError> {
    let Answer::Json(status,body)=answer else {return Ok(answer)};
    // Idempotency uses JSON.generate, not the HTML-escaping render-json encoder.
    let value:Value=serde_json::from_str(&body).map_err(|e|WebError::Config(format!("invalid response: {e}")))?;
    let body=value.to_string();
    c.execute("UPDATE idempotency_keys SET state='completed',response_status=?1,response_body=?2,updated_at=?3 WHERE id=?4",(i64::from(status.as_u16()),&body,crate::codec::format_time(app.now()),id))?;
    Ok(Answer::Json(status,body))
}

/// A claimed key given back unused: this build did nothing for the request (its `not_ported`), so the key is free for
/// the same request once a build serves it.
pub(super) fn release(c:&Connection,id:i64)->Result<(),WebError> {
    c.execute("DELETE FROM idempotency_keys WHERE id=?1 AND state='in_progress'",[id])?;
    Ok(())
}

/// BotsController's fingerprint payload (its override): the query, the body over it, `_action` and the path's bot id.
pub(super) fn bots_payload(input:&Input<'_>,action:&str)->Result<Map<String,Value>,WebError> {
    let mut payload=super::rest::object(super::bot::action_params::form(&input.params.query).map_err(|_|WebError::Config("invalid query".into()))?);
    payload.extend(input.body().map_err(|_|WebError::Config("invalid body".into()))?);
    payload.insert("_action".into(),json!(action));
    payload.insert("_bot_id".into(),json!(input.path.iter().find(|(k,_)|*k=="id").map(|(_,v)|v.as_str()).unwrap_or("")));
    Ok(payload)
}

/// The concern's default payload, `request.request_parameters`: the body alone, with ParamsWrapper's copy of it under the
/// controller's key (`wrap_parameters format: [:json]`, no model: every key but authenticity_token, _method and utf8)
/// when the body is JSON and no parameter already has that key.
pub(super) fn wrapped_payload(input:&Input<'_>,wrapper:&str,params:&Map<String,Value>)->Result<Map<String,Value>,WebError> {
    let mut payload=input.body().map_err(|_|WebError::Config("invalid body".into()))?;
    if input.is_json() && !params.contains_key(wrapper) {
        let copy:Map<String,Value>=payload.iter().filter(|(k,_)|!["authenticity_token","_method","utf8"].contains(&k.as_str())).map(|(k,v)|(k.clone(),v.clone())).collect();
        payload.insert(wrapper.into(),Value::Object(copy));
    }
    Ok(payload)
}

pub(super) fn run(c:&Connection,app:&App,user:i64,input:&Input<'_>,action:&str,work:impl FnOnce()->Result<Answer,WebError>)->Result<Answer,WebError> {
    match claim(c,app,user,input.idempotency,bots_payload(input,action)?)? {
        Claim::Answer(answer)=>Ok(answer),
        Claim::Claimed(id)=>complete(c,app,id,work()?),
    }
}
