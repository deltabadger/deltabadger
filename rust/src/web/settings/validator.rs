//! One paper-only validation boundary used by browser saves, rechecks and the scheduler.
use crate::web::WebError;
pub enum Validity {
    Correct,
    Incorrect,
    Pending(Diagnostic),
}
pub struct Diagnostic { pub text: String, pub status: Option<u16>, pub body: String }
impl Diagnostic {
    pub fn log_text(&self)->String { crate::venue::http::diagnostic(self.status,&self.body) }
}
// Retain numeric metadata only before the whole-text decision can discard a secret-bearing body.
fn log_body(body:&str)->String{
    match serde_json::from_str::<serde_json::Value>(body){
        Ok(value)=>match value.get("code").and_then(serde_json::Value::as_u64){Some(code)=>serde_json::json!({"code":code}).to_string(),None=>String::new()},
        Err(_)=>String::new(),
    }
}
fn pending(status:Option<u16>,body:&str,text:impl Into<String>)->Validity {
    Validity::Pending(Diagnostic{status,body:body.into(),text:text.into()})
}
pub async fn check(url: &str, key: &crate::crypto::Credentials, kind: i64) -> Result<Validity, WebError> {
    if key.passphrase.as_deref()==Some("live") {return Ok(Validity::Incorrect);}
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| WebError::Config("cannot initialize key validator".into()))?;
    let account = client
        .get(format!("{}/v2/account", url))
        .header("APCA-API-KEY-ID", &key.key)
        .header("APCA-API-SECRET-KEY", &key.secret)
        .send()
        .await;
    let response = match account {
        Ok(r) => r,
        Err(_) => return Ok(pending(None,"","validation request failed")),
    };
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|_| WebError::Config("cannot read key validation response".into()))?;
    let metadata=log_body(&body);
    let body=key.venue_text(&body);
    if status == 401 && kind!=2 {return Ok(Validity::Incorrect);}
    let value: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return Ok(pending(Some(status),&metadata,&body)),
    };
    if kind==2 && !(200..300).contains(&status) && value.get("message").and_then(|v|v.as_str()).unwrap_or(&body).contains("unauthorized"){return Ok(Validity::Incorrect);}
    if !(200..300).contains(&status) {
        return Ok(pending(Some(status),&metadata,
            value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or(&body),
        ));
    }
    if kind == 2 {
        let response = client
            .get(format!("{}/v2/positions", url))
            .header("APCA-API-KEY-ID", &key.key)
            .header("APCA-API-SECRET-KEY", &key.secret)
            .send()
            .await;
        let response = match response {
            Ok(r) => r,
            Err(_) => return Ok(pending(None,"","validation request failed")),
        };
        let status=response.status().as_u16();let successful=response.status().is_success();
        let text = response.text().await.map_err(|_| WebError::Config("cannot read positions validation".into()))?;
        let metadata=log_body(&text);
        let text=key.venue_text(&text);
        if !successful {
            let value=serde_json::from_str::<serde_json::Value>(&text).ok();
            let message=value.as_ref().and_then(|v|v.get("message")).and_then(|v|v.as_str()).unwrap_or(&text);
            return Ok(if message.contains("unauthorized"){Validity::Incorrect}else{pending(Some(status),&metadata,message)});
        }
        let positions=match serde_json::from_str::<serde_json::Value>(&text){
            Ok(serde_json::Value::Array(positions))=>positions,
            _=>return Ok(pending(Some(status),&metadata,"positions validation is unreadable")),
        };
        // Rails checks these after both calls, even for get_balances(asset_ids: []).
        if value.get("cash").is_none_or(serde_json::Value::is_null){
            return Ok(pending(Some(status),&metadata,"the account has no cash figure"));
        }
        if positions.iter().any(|position|matches!(position.get("asset_class").and_then(serde_json::Value::as_str),Some("us_equity"|"crypto"))
            && position.get("symbol").and_then(serde_json::Value::as_str).is_none_or(crate::ruby::blank)){
            return Ok(pending(Some(status),&metadata,"a position without a symbol"));
        }
        return Ok(Validity::Correct);
    }
    Ok(
        if value.get("status").and_then(|v| v.as_str()) == Some("ACTIVE") {
            Validity::Correct
        } else {
            Validity::Incorrect
        },
    )
}
