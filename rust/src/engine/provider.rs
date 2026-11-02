//! Index source attestation. Read Rails' encrypted configuration on the SAME connection and
//! inside the caller's write transaction. Configuration values and fingerprints never enter logs.
use super::{model::Bot, EngineError, staleness::Stale};
use crate::{app_config, crypto::Cipher, jobs::data_api::Config};
use chrono::{DateTime,Utc};
use rusqlite::{Connection, OptionalExtension, functions::FunctionFlags};
use sha2::{Digest,Sha256};
pub const KEY: &str = "rust_job.index_provider";
const MESSAGE: &str = "index needs a configured provider and a complete index refresh for its current configuration";

/// Register the existing cipher, not new key derivation, on this connection. Functions are
/// connection-local and callable from its transactions; no process-global cache of credentials.
pub fn bind(c: &Connection, cipher: &Cipher, env: &dyn Fn(&str)->Option<String>) -> Result<(),EngineError> {
    bind_cipher(c,cipher)?;
    let values=["MARKET_DATA_URL","MARKET_DATA_TOKEN","COINGECKO_API_KEY"].map(|k|(k,env(k)));
    c.create_scalar_function("rust_index_env",1,FunctionFlags::SQLITE_UTF8,move |cx| {
        let key:String=cx.get(0)?;
        Ok(values.iter().find(|(k,_)|*k==key).and_then(|(_,v)|v.clone()))
    })?;
    Ok(())
}
pub fn bind_cipher(c: &Connection, cipher: &Cipher) -> Result<(),EngineError> {
    let cipher=cipher.clone();
    c.create_scalar_function("rust_index_decrypt",1,FunctionFlags::SQLITE_UTF8,move |cx| {
        let raw:Option<String>=cx.get(0)?;
        Ok(raw.and_then(|s|cipher.decrypt(&s).ok()))
    })?;
    Ok(())
}
fn env(c:&Connection,key:&str)->Option<String> {
    c.query_row("SELECT rust_index_env(?1)",[key],|r|r.get::<_,Option<String>>(0))
        .unwrap_or_else(|_|std::env::var(key).ok())
}
fn value(c:&Connection,key:&str)->Result<Option<Option<String>>,EngineError> {
    let row:Option<Option<String>>=c.query_row("SELECT value FROM app_configs WHERE key=?1",[key],|r|r.get(0)).optional()?;
    row.map(|raw| {
        let decrypted=c.query_row("SELECT rust_index_decrypt(?1)",[raw.as_deref()],|r|r.get::<_,Option<String>>(0));
        // Plaintext is supported by Rails. An encrypted setting without an installed cipher
        // fails closed (read-only `check` can supply SECRET_KEY_BASE to decrypt it).
        Ok(decrypted.unwrap_or_else(|_|raw.filter(|v|!v.trim_start().starts_with('{'))))
    }).transpose()
}
/// The install's market-data settings (market_data::decide), read on this connection inside the caller's transaction.
pub fn settings(c:&Connection)->Result<crate::market_data::Settings,EngineError> {
    let row=|k:&str| value(c,k).map(|v| v.map(Option::unwrap_or_default));
    let rows=crate::market_data::Rows{provider:row("market_data_provider")?,url:row("market_data_url")?,token:row("market_data_token")?,coingecko_key:row("coingecko_api_key")?};
    let env=crate::market_data::Env{url:env(c,"MARKET_DATA_URL"),token:env(c,"MARKET_DATA_TOKEN"),coingecko_key:env(c,"COINGECKO_API_KEY")};
    Ok(crate::market_data::decide(&rows,&env))
}
pub fn config(c:&Connection)->Result<Option<Config>,EngineError> {
    Ok(settings(c)?.data_api_complete().cloned())
}
/// Direct CoinGecko is supported for the Top Coins ranking. Never fall back from a configured
/// data-api to a different provider, including when its credentials are blank.
pub fn coingecko_key(c: &Connection) -> Result<Option<String>, EngineError> {
    Ok(settings(c)?.coingecko_key().map(str::to_string))
}

/// Hash effective credentials and raw configuration rows/timestamps. Removing a row or changing
/// credentials invalidates freshness even when a previous job's success is recent.
pub fn fingerprint(c:&Connection)->Result<Option<String>,EngineError> {
    let config = match config(c)? {
        Some(config) => config,
        None => match coingecko_key(c)? { Some(key) => Config { url: "coingecko:top-coins".into(), token: key }, None => return Ok(None) },
    };
    let mut hash=Sha256::new();
    hash.update(serde_json::to_vec(&(config.url,config.token)).map_err(|_|EngineError::Data(MESSAGE.into()))?);
    let mut s=c.prepare("SELECT key,value,updated_at FROM app_configs WHERE key IN ('market_data_provider','market_data_url','market_data_token','coingecko_api_key') ORDER BY key")?;
    for row in s.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?)))? {
        hash.update(serde_json::to_vec(&row?).map_err(|_|EngineError::Data(MESSAGE.into()))?);
    }
    Ok(Some(hex::encode(hash.finalize())))
}
pub fn record(c:&Connection, fingerprint:&str, now:DateTime<Utc>)->Result<(),EngineError> {
    app_config::set_plain(c,KEY,fingerprint,now).map_err(EngineError::Data)
}
pub fn stale(c:&Connection,bot:&Bot)->Result<Option<Stale>,EngineError> {
    if bot.bot_type!="Bots::DcaIndex" {return Ok(None);}
    if config(c)?.is_none() && bot.index_external_id() != "top-coins" { return Ok(Some(Stale{source:"Index provider",message:MESSAGE.into()})); }
    let current=fingerprint(c)?;
    let recorded=app_config::get_plain(c,KEY).map_err(EngineError::Data)?;
    Ok((current.is_none() || current!=recorded).then(||Stale{source:"Index provider",message:MESSAGE.into()}))
}
