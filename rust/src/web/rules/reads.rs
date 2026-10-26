//! Live reads use the withdrawal key and V1's destination contract, including the full selection key.
use super::model::Rule;
use crate::{
    venue::{
        self,
        withdrawal::{self, Coin, Destination, Entry, Kind, Stored},
        Venue,
    },
    web::{App, WebError},
};
use serde_json::{json, Value};

pub struct Listing {
    pub entries: Vec<Entry>,
    pub networks: Option<Coin>,
    pub version: crate::engine::model::CredentialVersion,
    pub code: String,
}

pub async fn listing(app: &App, rule: &Rule, networks: bool) -> Result<Option<Listing>, WebError> {
    let (inner, user, exchange, asset) =
        (app.clone(), rule.user_id, rule.exchange.id, rule.asset.id);
    let require_correct = networks;
    let captured = app
        .db(move |c| {
            let tx = c.unchecked_transaction()?;
            let c = &tx;
            let Some((key, id)) =
                crate::engine::model::withdrawal_credentials(c, &inner.cipher, user, exchange)?
            else {
                return Ok(None);
            };
            let Some(code) = withdrawal::venue_code(c, exchange, asset)? else {
                return Ok(None);
            };
            if require_correct
                && !c.query_row("SELECT status=1 FROM api_keys WHERE id=?1", [id], |r| {
                    r.get::<_, bool>(0)
                })?
            {
                return Ok(None);
            }
            let Ok((_, version)) = crate::sync::credentials_with_version(c, &inner.cipher, id)
            else {
                return Ok(None);
            };
            Ok(Some((key, version, code)))
        })
        .await?;
    let Some((credentials, version, code)) = captured else {
        return Ok(None);
    };
    let Some(kind) = Kind::of(&rule.exchange.class) else {
        return Ok(None);
    };
    let Some(venue) = client(app, &rule.exchange.class, credentials).await? else {
        return Ok(None);
    };
    let mut entries = match venue.withdrawal_addresses(&code).await {
        Ok(entries) => entries,
        Err(_) => return Ok(None),
    };
    // V1 retains refused entries for diagnostics. Display reads follow Rails' active/verified
    // address books, so an entirely refused list renders the empty-wallet state.
    if !networks {
        entries.retain(|entry| entry.refused.is_none());
    }
    let coin = if networks && matches!(kind, Kind::Binance | Kind::Mexc) {
        match venue.withdrawal_networks(&code).await {
            Ok(coin) => coin,
            Err(_) => return Ok(None),
        }
    } else {
        None
    };
    let result = Some(Listing {
        entries,
        networks: coin,
        version: version.clone(),
        code,
    });
    // A replaced key cannot authorize a destination using an answer from the old key.
    let current = app
        .db(move |c| Ok(crate::engine::model::credential_is_current(c, &version)?))
        .await?;
    Ok(if current { result } else { None })
}

pub fn select(rule: &Rule, listing: &Listing) -> Result<Destination, withdrawal::Refusal> {
    let kind = Kind::of(&rule.exchange.class).ok_or(withdrawal::Refusal::NotListed)?;
    withdrawal::select(
        kind,
        &listing.code,
        &Stored::new(
            &rule.address,
            rule.settings["network"].as_str(),
            rule.settings["address_tag"].as_str(),
            rule.settings["address_name"].as_str(),
        ),
        &listing.entries,
        listing.networks.as_ref(),
    )
}

/// Store the exact destination V1 selected, without inventing a network or memo.
pub fn store(settings: &mut Value, destination: &Destination) {
    let mut set = |key: &str, value: Value| {
        if settings[key] != value {
            settings[key] = value;
        }
    };
    match destination {
        Destination::Binance { network, tag, .. } => {
            set("network", json!(network));
            set("address_tag", json!(tag));
        }
        Destination::Mexc { network, memo, .. } => {
            set("network", json!(network));
            set("address_tag", json!(memo));
        }
        Destination::Kraken { key, .. } => {
            set("address_name", json!(key));
        }
        Destination::Gemini { network, .. } => {
            set("network", json!(network));
        }
    }
}

// Reuse the venue egress settings; a broken proxy cannot silently become a direct request.
async fn client(
    app: &App,
    class: &str,
    mut credentials: crate::crypto::Credentials,
) -> Result<Option<venue::live::LiveVenue>, WebError> {
    let Some(kind) = Kind::of(class) else {
        return Ok(None);
    };
    let (inner, name) = (app.clone(), crate::tracker::rows::name_id(class));
    let egress = app
        .db(move |c| {
            Ok(venue::live::egress(
                c,
                &inner.cipher,
                &|k| std::env::var(k).ok(),
                &name,
                &venue::http::client(),
            ))
        })
        .await?;
    let Ok((http, proxy)) = egress else {
        return Ok(None);
    };
    credentials.redaction_values.extend(proxy);
    let transport = match kind {
        Kind::Binance => venue::http::ReqwestTransport::binance(
            http,
            credentials.key.clone(),
            credentials.secret.clone(),
        ),
        Kind::Kraken => venue::http::ReqwestTransport::new(
            http,
            credentials.key.clone(),
            credentials.secret.clone(),
        )
        .kraken(),
        Kind::Gemini => venue::http::ReqwestTransport::gemini(
            http,
            credentials.key.clone(),
            credentials.secret.clone(),
        ),
        Kind::Mexc => venue::http::ReqwestTransport::mexc(
            http,
            credentials.key.clone(),
            credentials.secret.clone(),
        ),
    }
    .with_clock(app.clock.clone())
    .with_sensitive_values(credentials.redaction_values);
    let base = app.venue_key_url(class);
    use venue::live::LiveVenue;
    Ok(Some(match kind {
        Kind::Binance => LiveVenue::Binance(venue::binance::BinanceVenue::new(
            transport,
            &base,
            &credentials.secret,
        )),
        Kind::Kraken => {
            LiveVenue::Kraken(venue::kraken::KrakenVenue::new(transport).with_base(&base))
        }
        Kind::Gemini => LiveVenue::Gemini(venue::gemini::GeminiVenue::new(transport, &base)),
        Kind::Mexc => LiveVenue::Mexc(venue::mexc::MexcVenue::new(
            transport,
            &base,
            &credentials.secret,
        )),
    }))
}

/// The wizard refreshes a stale display fee. Quotes are indicative at the entered minimum (default 0.1);
/// E0 must quote the actual budget again. V1 owns validation, Q1's fee key fallback, and D-RULE-FEE-STAMP.
pub async fn refresh_fee(
    app: &App,
    rule: &mut Rule,
    list: Option<&Listing>,
) -> Result<(), WebError> {
    use rusqlite::OptionalExtension;
    use withdrawal::FeeKey;
    let Some(kind) = Kind::of(&rule.exchange.class) else {
        return Ok(());
    };
    let (inner, user, exchange, asset, now) = (
        app.clone(),
        rule.user_id,
        rule.exchange.id,
        rule.asset.id,
        app.now(),
    );
    let captured = app.db(move |c| {
        let tx = c.unchecked_transaction()?;
        let c = &tx;
        let stamp: Option<String> = c.query_row("SELECT withdrawal_fee_updated_at FROM exchange_assets WHERE exchange_id=?1 AND asset_id=?2",
            (exchange, asset), |r| r.get(0)).optional()?.flatten();
        if stamp.and_then(|s| crate::codec::parse_time(&s).ok()).is_some_and(|at| at > now - chrono::Duration::hours(24)) { return Ok(None) }
        let key = if matches!(kind, Kind::Binance | Kind::Mexc) { withdrawal::fee_key(c, &inner.cipher, user, exchange)? }
            else { crate::engine::model::withdrawal_credentials(c, &inner.cipher, user, exchange)?.map(|(cr,id)| FeeKey::Withdrawal(cr,id)) };
        let Some(key) = key else { return Ok(None) };
        let version = if let FeeKey::Withdrawal(_, id) = &key {
            let Ok((_,version))=crate::sync::credentials_with_version(c,&inner.cipher,*id) else {return Ok(None)};
            Some(version)
        } else { None };
        let Some(code) = withdrawal::venue_code(c, exchange, asset)? else { return Ok(None) };
        Ok(Some((key, version, code)))
    }).await?;
    let Some((key, version, code)) = captured else {
        return Ok(());
    };
    let credentials = match &key {
        FeeKey::Fee(cr) | FeeKey::Withdrawal(cr, _) => cr.clone(),
    };
    let Some(venue) = client(app, &rule.exchange.class, credentials.clone()).await? else {
        return Ok(());
    };
    let stamp = if matches!(kind, Kind::Binance | Kind::Mexc) {
        venue
            .withdrawal_networks(&code)
            .await
            .ok()
            .flatten()
            .as_ref()
            .and_then(withdrawal::coin_stamp)
    } else {
        let Some(destination) = list.and_then(|l| select(rule, l).ok()) else {
            return Ok(());
        };
        let Some(amount) = crate::ruby::BigDec::parse(&rule.value("min_amount", "0.1"))
            .ok()
            .filter(crate::ruby::BigDec::is_positive)
        else {
            return Ok(());
        };
        venue
            .withdrawal_fee(&destination, &amount)
            .await
            .ok()
            .flatten()
            .as_ref()
            .map(withdrawal::quote_stamp)
    };
    let Some(stamp) = stamp else { return Ok(()) };
    let (inner, saved) = (app.clone(), stamp.clone());
    let written = app
        .db(move |c| {
            let tx = crate::engine::model::immediate(c)?;
            if let Some(version) = version {
                if !crate::engine::model::credential_is_current(&tx, &version)? {
                    return Ok(false);
                }
            } else {
                let Some(FeeKey::Fee(current)) =
                    withdrawal::fee_key(&tx, &inner.cipher, user, exchange)?
                else {
                    return Ok(false);
                };
                if current.key != credentials.key || current.secret != credentials.secret {
                    return Ok(false);
                }
            }
            let written = withdrawal::write_stamp(&tx, exchange, asset, &saved, now)?;
            tx.commit()?;
            Ok(written)
        })
        .await?;
    if written {
        rule.fee = crate::ruby::BigDec::parse(&stamp.fee).ok();
        if let Some(Value::Array(chains)) = stamp.chains {
            rule.chains = chains;
        }
    }
    Ok(())
}
