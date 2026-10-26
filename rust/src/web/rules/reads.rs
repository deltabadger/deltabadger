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

/// D-RULE-LISTED-ENTRY: one form value carries all of the venue's destination fields.
/// This is untrusted input; `select` checks it against a fresh listing and V1's contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub address: String,
    pub network: Option<String>,
    pub tag: Option<String>,
    pub key: Option<String>,
}

impl Identity {
    pub fn entry(entry: &Entry, kind: Kind) -> Self {
        Self {
            address: entry.address.clone(),
            network: match kind {
                Kind::Kraken => None,
                _ => entry.network.clone(),
            },
            tag: matches!(kind, Kind::Binance | Kind::Mexc)
                .then(|| entry.tag.clone())
                .flatten(),
            key: (kind == Kind::Kraken).then(|| entry.key.clone()).flatten(),
        }
    }

    pub fn rule(rule: &Rule) -> Self {
        let stored = Stored::new(
            &rule.address,
            rule.settings["network"].as_str(),
            rule.settings["address_tag"].as_str(),
            rule.settings["address_name"].as_str(),
        );
        let kraken = Kind::of(&rule.exchange.class) == Some(Kind::Kraken);
        Self {
            address: stored.address,
            network: if kraken { None } else { stored.network },
            tag: if matches!(
                Kind::of(&rule.exchange.class),
                Some(Kind::Binance | Kind::Mexc)
            ) {
                stored.tag
            } else {
                None
            },
            key: if kraken { stored.key } else { None },
        }
    }

    pub fn value(&self) -> String {
        json!({"address":self.address,"network":self.network,"tag":self.tag,"key":self.key})
            .to_string()
    }

    pub fn parse(value: &str) -> Result<Self, withdrawal::Refusal> {
        use withdrawal::Refusal::NotListed;
        let value: Value = serde_json::from_str(value).map_err(|_| NotListed)?;
        let fields = value.as_object().ok_or(NotListed)?;
        if fields.len() != 4 {
            return Err(NotListed);
        }
        let optional = |name: &str| match fields.get(name) {
            Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => Ok(Some(value.clone())),
            _ => Err(NotListed),
        };
        Ok(Self {
            address: fields
                .get("address")
                .and_then(Value::as_str)
                .ok_or(NotListed)?
                .to_string(),
            network: optional("network")?,
            tag: optional("tag")?,
            key: optional("key")?,
        })
    }

    pub fn listed(&self, rule: &Rule, listing: &Listing) -> bool {
        let Some(kind) = Kind::of(&rule.exchange.class) else {
            return false;
        };
        listing
            .entries
            .iter()
            .any(|e| e.refused.is_none() && Self::entry(e, kind) == *self)
    }

    pub fn apply(&self, address: &mut String, settings: &mut Value, kind: Kind) {
        address.clone_from(&self.address);
        match kind {
            Kind::Kraken => settings["address_name"] = json!(self.key),
            Kind::Binance | Kind::Mexc => {
                settings["network"] = json!(self.network);
                settings["address_tag"] = json!(self.tag);
            }
            Kind::Gemini => settings["network"] = json!(self.network),
        }
    }

    pub fn label(&self) -> String {
        if let Some(key) = &self.key {
            return key.clone();
        }
        let mut label = self.address.clone();
        if let Some(network) = &self.network {
            label += &format!(" ({network})");
        }
        if let Some(tag) = &self.tag {
            label += &format!(" — {tag}");
        }
        label
    }
}

pub struct Listing {
    pub entries: Vec<Entry>,
    pub networks: Option<Coin>,
    pub version: crate::engine::model::CredentialVersion,
    pub code: String,
}

/// Equivalent listed identities render once; only a unique match can be a form default.
pub fn picker<'a>(list: &'a Listing, rule: &Rule) -> (Vec<&'a Entry>, Option<usize>) {
    let kind = Kind::of(&rule.exchange.class).expect("withdrawal venue");
    let selected = Identity::rule(rule);
    let mut identities = Vec::new();
    let entries = list
        .entries
        .iter()
        .filter(|e| {
            if e.refused.is_some() {
                return false;
            }
            let identity = Identity::entry(e, kind);
            if identities.contains(&identity) {
                return false;
            }
            identities.push(identity);
            true
        })
        .collect::<Vec<_>>();
    let matches = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            let identity = Identity::entry(e, kind);
            e.refused.is_none()
                && identity.address == selected.address
                && selected.network.as_ref().is_none_or(|n| {
                    identity.network.as_ref() == Some(n)
                        || kind == Kind::Gemini
                            && identity.network.is_none()
                            && withdrawal::gemini_network(&list.code) == Some(n.as_str())
                        || kind == Kind::Mexc
                            && withdrawal::select(
                                kind,
                                &list.code,
                                &Stored::new(&rule.address, Some(n), selected.tag.as_deref(), None),
                                std::slice::from_ref(**e),
                                list.networks.as_ref(),
                            )
                            .is_ok()
                })
                && selected
                    .tag
                    .as_ref()
                    .is_none_or(|t| identity.tag.as_ref() == Some(t))
                && selected
                    .key
                    .as_ref()
                    .is_none_or(|k| identity.key.as_ref() == Some(k))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let selected_index = match matches.as_slice() {
        [index] => Some(*index),
        _ => None,
    };
    (entries, selected_index)
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
    // MEXC may list a code while V1 stores the coin list's network label. Display
    // reads also need this mapping to select the destination and preview its chain fee.
    let needs_aliases = kind == Kind::Mexc;
    let coin = if (networks && matches!(kind, Kind::Binance | Kind::Mexc)) || needs_aliases {
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
