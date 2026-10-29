//! A12 PR 1: pure identity and conservative legacy matching. No live writer calls this module.
//! Callers must supply a complete collision bucket, scoped to (user_id, exchange_id), never API key.
//! A Missing verdict is evidence only: PR 2 must still fence writes, check suppressions and aliases,
//! assign anchors, preserve existing rows, and commit whole dust pairs with the install marker.
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Source { Deposit, Withdrawal, FlexibleReward, LockedReward, FlexibleSubscription, FlexibleRedemption,
    LockedSubscription, LockedRedemption, DustOut, DustIn }
impl Source {
    fn name(self) -> &'static str { match self {
        Self::Deposit=>"deposit",Self::Withdrawal=>"withdrawal",Self::FlexibleReward=>"flex-reward",Self::LockedReward=>"lock-reward",
        Self::FlexibleSubscription=>"flex-sub",Self::FlexibleRedemption=>"flex-redeem",Self::LockedSubscription=>"lock-sub",
        Self::LockedRedemption=>"lock-redeem",Self::DustOut=>"dust-out",Self::DustIn=>"dust-in" } }
    fn dust(self) -> bool { matches!(self,Self::DustOut|Self::DustIn) }
    fn reward(self) -> bool { matches!(self,Self::FlexibleReward|Self::LockedReward) }
}
#[derive(Clone, Copy, Default)]
pub struct Context<'a> { pub parent: Option<&'a str>, pub requested_type: Option<&'a str>, pub occurred_ms: Option<i64> }
/// Retain the encoded tuple alongside every digest; equality never trusts the digest alone.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Key(String);
impl Key {
    fn new(source: &str, fields: &[String]) -> Self { Self(serde_json::to_string(&("a12-v1",source,fields)).expect("string tuple")) }
    pub fn tuple(&self) -> &str { &self.0 }
    pub fn digest(&self) -> String { hex::encode(Sha256::digest(self.0.as_bytes())) }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Event { source: Source, key: Option<Key>, pair: Option<Key>, evidence: Vec<Option<String>>, payload: Option<Value> }
fn text(raw: &Value, field: &str) -> Option<String> {
    match &raw[field] { Value::String(s) if !s.trim().is_empty()=>Some(s.clone()),Value::Number(n) if n.is_i64()||n.is_u64()=>Some(n.to_string()),_=>None }
}
impl Event {
    /// Reconstruct a legacy candidate using the incoming endpoint context. The resolver must
    /// require a unique match before retaining this context (especially a dust parent).
    pub(crate) fn legacy(&self, raw: &Value, at: i64) -> Result<Self, &'static str> {
        let pair: Value = self.pair.as_ref().map(|p|serde_json::from_str(p.tuple())).transpose().map_err(|_|"unreadable retained pair tuple")?.unwrap_or(Value::Null);
        Self::read(self.source,raw,Context {parent:pair[2][0].as_str(),requested_type:raw["type"].as_str(),occurred_ms:Some(at)})
    }
    pub(crate) fn equivalent(&self, other: &Self) -> bool { self.same(other) }
    pub(crate) fn capital(&self) -> bool { matches!(self.source,Source::Deposit|Source::Withdrawal) }
    /// Original raw_data is borrowed, never rewritten. Missing stable IDs cannot mint a fallback key.
    pub fn read(source: Source, raw: &Value, context: Context<'_>) -> Result<Self, &'static str> {
        let required=|k| text(raw,k).ok_or("missing identity evidence");
        let decimal=|k| super::number::decimal(&required(k)?,&super::number::VENUE).map(|d|d.to_s_f()).map_err(|_|"invalid exact decimal");
        let time=|k| required(k)?.parse::<i64>().map(|t|t.to_string()).map_err(|_|"invalid integer milliseconds");
        let tuple=|fields:&[&str]| fields.iter().map(|f|text(raw,f)).collect::<Option<Vec<_>>>();
        let known=|values:Vec<String>| values.into_iter().map(Some).collect::<Vec<_>>();
        let mut pair=None;
        let (fields,evidence)=match source {
            Source::Deposit|Source::Withdrawal=> {
                let at=if source==Source::Deposit {time("insertTime")?} else {
                    chrono::NaiveDateTime::parse_from_str(&required("applyTime")?,"%Y-%m-%d %H:%M:%S")
                        .map_err(|_|"invalid withdrawal time")?.and_utc().timestamp_millis().to_string()
                };
                let mut evidence=vec![required("txId")?,required("coin")?,required("network")?,required("address")?,
                    raw["addressTag"].as_str().ok_or("missing transfer tag evidence")?.into(),decimal("amount")?,at];
                if source==Source::Withdrawal { evidence.push(decimal("transactionFee")?); }
                (tuple(&["id"]),known(evidence))
            }
            Source::FlexibleReward|Source::LockedReward=> {
                let asset=required("asset")?; let at=time("time")?;
                let discriminator=if source==Source::FlexibleReward {"projectId"} else {"positionId"};
                let mut fields=tuple(&[discriminator]);
                let mut evidence=known(vec![asset.clone(),at.clone(),
                    decimal(if raw.get("rewards").is_some_and(|v|!v.is_null()) {"rewards"} else {"amount"})?]);
                evidence.insert(0,text(raw,discriminator));
                if source==Source::FlexibleReward {
                    let kind=required("type")?;
                    if context.requested_type!=Some(kind.as_str()) {return Err("reward type disagrees with query")}
                    evidence.push(Some(kind.clone())); if let Some(fields)=&mut fields {fields.push(kind);}
                }
                if let Some(fields)=&mut fields {fields.extend([asset,at]);}
                (fields,evidence)
            }
            Source::DustOut|Source::DustIn=> {
                let fields=context.parent.filter(|p|!p.trim().is_empty()).and_then(|p|tuple(&["transId","fromAsset"]).map(|mut f|{f.insert(0,p.into());f}));
                pair=fields.as_ref().map(|f|Key::new("dust",f));
                (fields,known(vec![required("transId")?,required("fromAsset")?,decimal("amount")?,decimal("transferedAmount")?,decimal("serviceChargeAmount")?,context.occurred_ms.ok_or("missing dust parent time")?.to_string()]))
            }
            _=> {
                let subscription=matches!(source,Source::FlexibleSubscription|Source::LockedSubscription);
                let amount=if subscription && raw.get("purchaseAmount").is_some_and(|v|!v.is_null()) {"purchaseAmount"} else {"amount"};
                // Binance's flexible subscription response uses productId, not projectId:
                // https://raw.githubusercontent.com/binance/binance-connector-python/master/clients/simple_earn/src/binance_sdk_simple_earn/rest_api/models/get_flexible_subscription_record_response_rows_inner.py
                let product_field=match source {
                    Source::FlexibleSubscription=>"productId",
                    Source::FlexibleRedemption|Source::LockedSubscription=>"projectId",
                    Source::LockedRedemption=>"positionId",
                    _=>unreachable!(),
                };
                let product=text(raw,product_field).ok_or("missing Earn product evidence")?;
                (tuple(&[if subscription {"purchaseId"} else {"redeemId"}]),vec![Some(product),
                    if source==Source::LockedSubscription {text(raw,"positionId")} else {None},Some(required("asset")?),
                    Some(time(if raw.get("time").is_some_and(|v|!v.is_null()) {"time"} else {"createTime"})?),Some(decimal(amount)?)])
            }
        };
        // No-ID reward tuples are not universally unique. Compare the retained payload on a
        // tuple match: known differences conflict, while added/missing fields remain unresolved.
        let payload=if source.reward() {
            let mut payload=raw.clone();
            for field in ["amount","rewards"] {if raw.get(field).is_some_and(|v|!v.is_null()) {payload[field]=Value::String(decimal(field)?);}}
            payload["time"]=Value::String(time("time")?);
            for field in ["asset","type","projectId","positionId"] {
                if let Some(value)=text(raw,field) {payload[field]=Value::String(value);}
                else if matches!(field,"projectId"|"positionId") {payload.as_object_mut().unwrap().remove(field);}
            }
            Some(payload)
        } else {None};
        Ok(Self {source,key:fields.map(|f|Key::new(source.name(),&f)),pair,evidence,payload})
    }
    pub fn key(&self) -> Option<&Key> { self.key.as_ref() }
    pub fn pair_key(&self) -> Option<&Key> { self.pair.as_ref() }
    pub fn group_id(&self) -> Option<String> { self.pair.as_ref().map(|p|format!("dust_a12_v1_{}",p.digest())) }
    pub fn sibling_id(&self) -> Option<String> {
        if let Some(pair)=&self.pair {Some(format!("dust-a12-v1-{}-{}",pair.digest(),if self.source==Source::DustIn {"in"}else{"out"}))}
        else {self.key.as_ref().map(|k|format!("binance-a12-v1:{}:{}",self.source.name(),k.digest()))}
    }
    fn same(&self,other:&Self)->bool {self.source==other.source && self.key==other.key && self.evidence==other.evidence && self.payload==other.payload}
    /// Only values present on both sides can prove a correction. Missing evidence
    /// remains uncertain, including fields added to a historical reward payload.
    fn differs(&self,other:&Self)->bool {
        self.evidence.iter().zip(&other.evidence).any(|(a,b)|matches!((a,b),(Some(a),Some(b)) if a!=b)) ||
            match (&self.payload,&other.payload) {
                (Some(Value::Object(a)),Some(Value::Object(b)))=>a.iter().any(|(k,v)|
                    !v.is_null() && b.get(k).is_some_and(|w|!w.is_null() && v!=w)),
                _=>false,
            }
    }
    fn possible(&self,other:&Self)->bool {
        if self.source!=other.source {return false}
        if let (Some(a),Some(b))=(&self.key,&other.key) {return a==b}
        // With an incomplete key, compare known identity evidence independently
        // of economics so a changed amount or fee still reaches correction detection.
        let identity:&[usize]=match self.source {
            Source::Deposit|Source::Withdrawal=>&[0,1,2,3,4,6], // transfer route and time
            Source::FlexibleReward=>&[0,1,2,4], // project, asset, time, type
            Source::LockedReward=>&[0,1,2], // position, asset, time
            Source::DustOut|Source::DustIn=>&[0,1,5], // detail, asset, time
            _=>&[0,1,2,3], // product, optional position, asset, time
        };
        identity.iter().all(|&i|match (&self.evidence[i],&other.evidence[i]) {
            (Some(a),Some(b))=>a==b,_=>true,
        })
    }
}
/// A missing/unreadable/deleted raw identity is None; nil-ID/file candidates cannot establish an anchor.
pub struct Candidate { pub row_id: i64, pub event: Option<Event>, pub legacy_id: bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision { Existing(i64), Missing, Conflict, Unresolved }

pub fn classify(incoming: &[Event], stored: &[Candidate]) -> Vec<Decision> {
    if stored.iter().filter_map(|c|c.event.as_ref()).filter(|e|e.source.dust()).any(|e|
        stored.iter().filter_map(|c|c.event.as_ref()).filter(|other|other.source.dust() && other.source!=e.source &&
            e.evidence==other.evidence && (e.pair.is_none() || other.pair.is_none() || e.pair==other.pair)).count()!=1) {
        return vec![Decision::Unresolved;incoming.len()];
    }
    let mut decisions:Vec<_>=incoming.iter().map(|event| {
        if stored.iter().any(|c|c.event.is_none()) {return Decision::Unresolved}
        let peers:Vec<_>=incoming.iter().filter(|p|event.possible(p)).collect();
        if peers.iter().any(|p|event.differs(p)) {return Decision::Conflict}
        if peers.iter().any(|p|!event.same(p)) {return Decision::Unresolved}
        if peers.len()>1 && (event.source.reward()||event.key.is_none()) {return Decision::Unresolved}
        let matches:Vec<_>=stored.iter().filter(|c|event.possible(c.event.as_ref().unwrap())).collect();
        if matches.len()>1 {return Decision::Unresolved}
        if let Some(c)=matches.first() {
            let prior=c.event.as_ref().unwrap();
            if event.differs(prior) {return Decision::Conflict}
            if !event.same(prior) {return Decision::Unresolved}
            if !c.legacy_id || incoming.iter().any(|other|prior.possible(other) && other.key!=event.key) {return Decision::Unresolved}
            return Decision::Existing(c.row_id)
        }
        if event.key.is_some() {Decision::Missing} else {Decision::Unresolved}
    }).collect();
    // A dust unit cannot be authorized partially, or matched to one retained leg and one new leg.
    for (i,event) in incoming.iter().enumerate().filter(|(_,e)|e.source.dust()) {
        let mates:Vec<_>=incoming.iter().enumerate().filter(|(_,other)|event.pair.is_some() && event.pair==other.pair).map(|(j,_)|j).collect();
        let has_both=mates.iter().any(|j|incoming[*j].source!=event.source);
        let consistent=mates.iter().all(|j|event.evidence==incoming[*j].evidence);
        let all_missing=mates.iter().all(|j|decisions[*j]==Decision::Missing);
        let all_existing=mates.iter().all(|j|matches!(decisions[*j],Decision::Existing(_)));
        if !has_both || !consistent || !(all_missing||all_existing) {decisions[i]=Decision::Unresolved;for j in mates {decisions[j]=Decision::Unresolved;}}
    }
    decisions
}
