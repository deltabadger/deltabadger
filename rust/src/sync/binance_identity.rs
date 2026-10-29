//! A12 PR 1: pure identity and conservative legacy matching. No live writer calls this module.
//! Callers must supply a complete collision bucket, scoped to (user_id, exchange_id), never API key.
//! A Missing verdict is evidence only: PR 2 must still fence writes, check suppressions and aliases,
//! assign anchors, preserve existing rows, and commit whole dust pairs with the install marker.
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key(String);
impl Key {
    fn new(source: &str, fields: &[String]) -> Self { Self(serde_json::to_string(&("a12-v1",source,fields)).expect("string tuple")) }
    pub fn tuple(&self) -> &str { &self.0 }
    pub fn digest(&self) -> String { hex::encode(Sha256::digest(self.0.as_bytes())) }
}
#[derive(Clone, Debug)]
pub struct Event { source: Source, key: Option<Key>, pair: Option<Key>, evidence: Vec<String>, payload: Option<Value> }
fn text(raw: &Value, field: &str) -> Option<String> {
    match &raw[field] { Value::String(s) if !s.trim().is_empty()=>Some(s.clone()),Value::Number(n) if n.is_i64()||n.is_u64()=>Some(n.to_string()),_=>None }
}
impl Event {
    /// Original raw_data is borrowed, never rewritten. Missing stable IDs cannot mint a fallback key.
    pub fn read(source: Source, raw: &Value, context: Context<'_>) -> Result<Self, &'static str> {
        let required=|k| text(raw,k).ok_or("missing identity evidence");
        let decimal=|k| super::number::decimal(&required(k)?,&super::number::VENUE).map(|d|d.to_s_f()).map_err(|_|"invalid exact decimal");
        let time=|k| required(k)?.parse::<i64>().map(|t|t.to_string()).map_err(|_|"invalid integer milliseconds");
        let tuple=|fields:&[&str]| fields.iter().map(|f|text(raw,f)).collect::<Option<Vec<_>>>();
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
                (tuple(&["id"]),evidence)
            }
            Source::FlexibleReward|Source::LockedReward=> {
                let asset=required("asset")?; let at=time("time")?;
                let discriminator=if source==Source::FlexibleReward {"projectId"} else {"positionId"};
                let mut fields=tuple(&[discriminator]);
                let mut evidence=vec![text(raw,discriminator).unwrap_or_default(),asset.clone(),at.clone(),
                    decimal(if raw.get("rewards").is_some_and(|v|!v.is_null()) {"rewards"} else {"amount"})?];
                if source==Source::FlexibleReward {
                    let kind=required("type")?;
                    if context.requested_type!=Some(kind.as_str()) {return Err("reward type disagrees with query")}
                    evidence.push(kind.clone()); if let Some(fields)=&mut fields {fields.push(kind);}
                }
                if let Some(fields)=&mut fields {fields.extend([asset,at]);}
                (fields,evidence)
            }
            Source::DustOut|Source::DustIn=> {
                let fields=context.parent.filter(|p|!p.trim().is_empty()).and_then(|p|tuple(&["transId","fromAsset"]).map(|mut f|{f.insert(0,p.into());f}));
                pair=fields.as_ref().map(|f|Key::new("dust",f));
                (fields,vec![required("transId")?,required("fromAsset")?,decimal("amount")?,decimal("transferedAmount")?,decimal("serviceChargeAmount")?,context.occurred_ms.ok_or("missing dust parent time")?.to_string()])
            }
            _=> {
                let subscription=matches!(source,Source::FlexibleSubscription|Source::LockedSubscription);
                let amount=if subscription && raw.get("purchaseAmount").is_some_and(|v|!v.is_null()) {"purchaseAmount"} else {"amount"};
                let product=text(raw,"projectId").or_else(||text(raw,"positionId")).ok_or("missing Earn product evidence")?;
                (tuple(&[if subscription {"purchaseId"} else {"redeemId"}]),vec![product,text(raw,"positionId").unwrap_or_default(),text(raw,"projectId").unwrap_or_default(),required("asset")?,
                    time(if raw.get("time").is_some_and(|v|!v.is_null()) {"time"} else {"createTime"})?,decimal(amount)?])
            }
        };
        // No-ID reward tuples are not universally unique. Compare the retained payload on a
        // tuple match; a changed payload is a conflict, never another key or a silent duplicate.
        let payload=if source.reward() {
            let mut payload=raw.clone();
            for field in ["amount","rewards"] {if raw.get(field).is_some_and(|v|!v.is_null()) {payload[field]=Value::String(decimal(field)?);}}
            payload["time"]=Value::String(time("time")?);
            for field in ["asset","type","projectId","positionId"] {if let Some(value)=text(raw,field) {payload[field]=Value::String(value);}}
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
    fn same(&self,other:&Self)->bool {self.source==other.source && self.evidence==other.evidence && self.payload==other.payload}
    fn possible(&self,other:&Self)->bool {
        self.source==other.source && match (&self.key,&other.key) { (Some(a),Some(b))=>a==b,_=>self.same(other) }
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
        if peers.iter().any(|p|!event.same(p)) {return Decision::Conflict}
        if peers.len()>1 && (event.source.reward()||event.key.is_none()) {return Decision::Unresolved}
        let matches:Vec<_>=stored.iter().filter(|c|event.possible(c.event.as_ref().unwrap())).collect();
        if matches.len()>1 {return Decision::Unresolved}
        if let Some(c)=matches.first() {
            let prior=c.event.as_ref().unwrap();
            if !event.same(prior) {return Decision::Conflict}
            if !c.legacy_id || incoming.iter().any(|other|prior.possible(other) && other.key!=event.key) {return Decision::Unresolved}
            return Decision::Existing(c.row_id)
        }
        if event.key.is_some() {Decision::Missing} else {Decision::Unresolved}
    }).collect();
    // A dust unit cannot be authorized partially, or matched to one retained leg and one new leg.
    for (i,event) in incoming.iter().enumerate().filter(|(_,e)|e.source.dust()) {
        let mates:Vec<_>=incoming.iter().enumerate().filter(|(_,other)|event.pair.is_some() && event.pair==other.pair).map(|(j,_)|j).collect();
        let has_both=mates.iter().any(|j|incoming[*j].source!=event.source);
        let all_missing=mates.iter().all(|j|decisions[*j]==Decision::Missing);
        let all_existing=mates.iter().all(|j|matches!(decisions[*j],Decision::Existing(_)));
        if !has_both || !(all_missing||all_existing) {decisions[i]=Decision::Unresolved;for j in mates {decisions[j]=Decision::Unresolved;}}
    }
    decisions
}
