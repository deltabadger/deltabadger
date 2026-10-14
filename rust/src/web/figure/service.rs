//! At most two fills, one per account, and no decimal crosses this boundary. Who waits for a publication (`want`) is
//! kept here with that account's own allowance of fills, so that a fill that is turned away or overtaken leaves its
//! demand behind, and waiting behind another account never spends it. Figures work for the `figures` service is queued
//! here too, one entry per account (`enqueue`), with what its CPU work runs on (`Workers`).
use crate::figures::page_market::{Cache,FxDeps};
use std::collections::{BTreeMap,BTreeSet};
use std::sync::{Arc,Mutex,MutexGuard};
use tokio::sync::Notify;

const ACCOUNTS:usize=8;
/// Fills one waiting account may start before its wait ends in no value: the first, and two overtaken by writes.
pub const ALLOWANCE:u8=3;
#[derive(Default,Clone)]
pub struct Service(Arc<Mutex<State>>,Arc<Notify>,Arc<Workers>);
/// What the figures' CPU work runs on: one permit for all of it (a fill's decoding and merging, its computations, the
/// queue's render-only passes), held by the worker until its closure returns; the builder's own connection, never the
/// one pages use; how long a fill may run.
struct Workers { compute:Arc<tokio::sync::Semaphore>, builder:Mutex<Builder>, timeout:Mutex<std::time::Duration> }
type BuilderHook=Box<dyn FnOnce(&rusqlite::Connection)+Send>;
#[derive(Default)]
struct Builder { connection:Option<rusqlite::Connection>, path:Option<String>, hooks:Vec<BuilderHook>, logged:Vec<String> }
/// Readers never block a writer or each other only in WAL, which Rails sets and this build leaves alone (`store::configure`).
const NOT_WAL:&str="figures: database not in WAL; page latency during background work is not guaranteed";
impl Default for Workers {
    fn default()->Self { Workers{ compute:Arc::new(tokio::sync::Semaphore::new(1)), builder:Mutex::default(), timeout:Mutex::new(std::time::Duration::from_secs(90)) } }
}
#[derive(Default)]
struct State { active:usize, active_users:BTreeSet<i64>, serial:u64, entries:BTreeMap<i64,Entry>, wanted:BTreeMap<i64,u8>, marked:BTreeSet<i64>, owners:BTreeSet<i64>, latest:BTreeMap<i64,Latest>,
    /// Work waiting for the `figures` service (`loading::follow`), one entry per account.
    pending:BTreeMap<i64,Work>,
    /// Work for an account whose fill is running: back to `pending` when that fill ends (`Ticket::store`).
    parked:BTreeMap<i64,Work>,
    /// When each account was last signed in on a request (Unix seconds).
    signed_in:BTreeMap<i64,i64>,
    /// An account with no paper Alpaca key: why its figures are absent, read by the queue from its stored fills, for the
    /// revision (`loading::revision`) it was read against, until the next cut.
    unkeyed:BTreeMap<i64,(u64,i64,Option<&'static str>)>,
    /// Conversions (from, to) a page found no held answer for: filled by the queue (`loading::follow`), whatever the
    /// account (a tracker-only account has no paper key, and its rate is wanted all the same).
    fx_wanted:BTreeSet<(String,String)> }
/// An account's last publication: the serial of the figures it rendered, whether all of it was kept, and (stream,
/// payload) one per target as a publication carries them, within the mailbox's bounds.
/// And the held conversions its figures read.
type Latest=(u64,bool,Vec<(Arc<str>,Arc<str>)>,FxDeps);
struct Entry { identity:String,revision:u64,serial:u64,started:i64,until:i64,loading:bool,ready:bool,reason:Option<&'static str>,cache:Cache }
/// `Busy`: both fills are taken by other accounts. `Failed` also when a fill is needed and the waiting account has no
/// allowance left.
/// `Ready`: the cache, when its fill began, and until when its figures may be shown (`Entry::until`).
pub enum Load { Cold, Failed(Option<&'static str>), Busy, Ready(Cache,i64,i64), Start(Ticket,Cache) }
pub struct Ticket { service:Service,user:i64,serial:u64,finished:bool }
/// Figures work for one account: publish them, and the (locale, path prefix) keys to render. Due from `not_before`.
#[derive(Clone,Debug,Default,PartialEq,Eq)]
pub struct Work { pub publish:bool, pub locales:BTreeSet<(String,String)>, pub not_before:i64 }
impl Work {
    /// One entry per account: what either asks for, due at the earlier time. A later enqueue never pushes it back.
    fn merge(&mut self,other:Work) { self.publish|=other.publish; self.locales.extend(other.locales); self.not_before=self.not_before.min(other.not_before); }
}
fn put(queue:&mut BTreeMap<i64,Work>,user:i64,work:Work) {
    match queue.entry(user) { std::collections::btree_map::Entry::Occupied(mut entry)=>entry.get_mut().merge(work), std::collections::btree_map::Entry::Vacant(entry)=>{entry.insert(work);} }
}
impl Service {
    /// Queues figures work for this account, merged with what it already waits for. While the account's fill runs the
    /// work is parked, and that fill's end queues it again: nothing asked for after a fill captured its work is lost.
    pub fn enqueue(&self,user:i64,work:Work) {
        let mut state=self.state();
        let parked=state.active_users.contains(&user);
        put(if parked { &mut state.parked } else { &mut state.pending },user,work);
        drop(state);
        self.1.notify_one();
    }
    /// A page wants a conversion it found no held answer for. The queue fills it; the page never fetches.
    pub fn want_fx(&self,from:&str,to:&str) {
        if self.state().fx_wanted.insert((from.to_string(),to.to_string())) { self.1.notify_one(); }
    }
    /// The wanted conversions, taken off the queue.
    pub fn take_fx(&self)->Vec<(String,String)> { std::mem::take(&mut self.state().fx_wanted).into_iter().collect() }
    /// The queued work due at `now`, taken off the queue.
    pub fn take_due(&self,now:i64)->Vec<(i64,Work)> {
        let mut state=self.state();
        let due:Vec<i64>=state.pending.iter().filter(|(_,w)|w.not_before<=now).map(|(user,_)|*user).collect();
        due.into_iter().filter_map(|user|state.pending.remove(&user).map(|w|(user,w))).collect()
    }
    /// The one permit for the figures' CPU work.
    pub fn compute(&self)->Arc<tokio::sync::Semaphore> { self.2.compute.clone() }
    /// How long a fill may run before its waiter stops (a started worker still finishes, holding its permit).
    pub fn fill_timeout(&self)->std::time::Duration { *self.2.timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner) }
    pub fn set_fill_timeout(&self,timeout:std::time::Duration) { *self.2.timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner)=timeout; }
    /// For tests: run on the builder's connection when it opens (at once if it is open).
    pub fn on_builder(&self,hook:impl FnOnce(&rusqlite::Connection)+Send+'static) {
        let mut builder=self.2.builder.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match &builder.connection { Some(c)=>hook(c), None=>builder.hooks.push(Box::new(hook)) }
    }
    /// Logs `line`, and keeps it among what the figures service said (`logged`).
    pub fn note(&self,line:&str) { crate::engine::log(line); self.2.builder.lock().unwrap_or_else(std::sync::PoisonError::into_inner).logged.push(line.into()); }
    /// What the figures service logged: the builder when it opened, and passes that could not render (`note`).
    pub fn logged(&self)->Vec<String> { self.2.builder.lock().unwrap_or_else(std::sync::PoisonError::into_inner).logged.clone() }
    /// The database file the builder opens; None until told (`loading::build`).
    pub fn builder_path(&self)->Option<String> { self.2.builder.lock().unwrap_or_else(std::sync::PoisonError::into_inner).path.clone() }
    pub fn set_builder_path(&self,path:String) { self.2.builder.lock().unwrap_or_else(std::sync::PoisonError::into_inner).path=Some(path); }
    /// `work` on the builder's connection, opened on first use with the install's settings (`store::configure`). Called on
    /// a blocking thread that holds the compute permit, so the lock is never contended.
    pub fn with_builder<T>(&self,work:impl FnOnce(&rusqlite::Connection)->Result<T,crate::web::WebError>)->Result<T,crate::web::WebError> {
        use crate::web::WebError;
        let mut builder=self.2.builder.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if builder.connection.is_none() {
            let path=builder.path.clone().ok_or_else(||WebError::Config("figures builder has no database".into()))?;
            let c=rusqlite::Connection::open(&path)?;
            crate::store::configure(&c)?;
            // Page latency during a fill holds only in WAL: said once, when the builder opens.
            if !c.query_row("PRAGMA journal_mode",[],|r|r.get::<_,String>(0))?.eq_ignore_ascii_case("wal") { crate::engine::log(NOT_WAL); builder.logged.push(NOT_WAL.into()); }
            for hook in std::mem::take(&mut builder.hooks) { hook(&c); }
            builder.connection=Some(c);
        }
        let Some(c)=builder.connection.as_ref() else { return Err(WebError::Config("figures builder unavailable".into())) };
        let out=work(c);
        if !c.is_autocommit() {
            c.execute_batch("ROLLBACK").map_err(|_|WebError::Config("figures builder rollback failed".into()))?;
            if out.is_ok() { return Err(WebError::Config("figures builder left an uncommitted transaction".into())); }
        }
        out
    }
    /// A signed-in request of this account (Bot::WarmMetricsCachesJob.mark_activity!), per account.
    pub fn mark_active(&self,user:i64,now:i64) { self.state().signed_in.insert(user,now); }
    /// The accounts signed in since `since`.
    pub fn active_since(&self,since:i64)->BTreeSet<i64> { let mut state=self.state(); state.signed_in.retain(|_,at|*at>=since); state.signed_in.keys().copied().collect() }
    /// The reason read for an account with no paper key, if read against this `revision` and still before its cut.
    pub fn unkeyed(&self,user:i64,revision:u64,now:i64)->Option<Option<&'static str>> {
        self.state().unkeyed.get(&user).filter(|(at,until,_)|*at==revision&&*until>now).map(|(_,_,reason)|*reason)
    }
    pub fn set_unkeyed(&self,user:i64,revision:u64,until:i64,reason:Option<&'static str>) { self.state().unkeyed.insert(user,(revision,until,reason)); }
    /// Whether a fill of this account is running.
    pub fn filling(&self,user:i64)->bool { self.state().active_users.contains(&user) }
    /// When the earliest queued work is due.
    pub fn next_due(&self)->Option<i64> { self.state().pending.values().map(|w|w.not_before).min() }
    fn state(&self)->MutexGuard<'_,State> { self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) }
    pub fn begin(&self,user:i64,identity:&str,revision:u64,now:i64)->Load {
        let mut state=self.state();
        // One fill per account, whatever revision it was started for: its end starts the next one if a page still waits.
        if state.entries.get(&user).is_some_and(|e|!crate::engine::model::identity_current_for(&e.identity,identity).is_fresh()) {
            state.entries.remove(&user);state.latest.remove(&user);
        }
        if state.active_users.contains(&user){return Load::Cold;}
        if let Some(entry)=state.entries.get(&user).filter(|e|crate::engine::model::identity_current_for(&e.identity,identity).is_fresh()&&e.revision==revision) {
            if entry.until>now { return if entry.ready { Load::Ready(entry.cache.clone(),entry.started,entry.until) } else { Load::Failed(entry.reason) }; }
        }
        if state.wanted.get(&user)==Some(&0) { return Load::Failed(state.entries.get(&user).and_then(|e|e.reason)); }
        // Waiting for capacity spends nothing.
        if state.active>=2 { return Load::Busy; }
        if state.entries.len()>=ACCOUNTS && !state.entries.contains_key(&user) {
            let victim=state.entries.iter().filter(|(_,e)|!e.loading).min_by_key(|(_,e)|e.started).map(|(id,_)|*id);
            if let Some(id)=victim { state.entries.remove(&id);state.latest.remove(&id); } else { return Load::Busy; }
        }
        if let Some(allowance)=state.wanted.get_mut(&user) { *allowance-=1; }
        let cache=state.entries.get(&user).filter(|e|crate::engine::model::identity_current_for(&e.identity,identity).is_fresh()).map(|e|e.cache.clone()).unwrap_or_default(); // allow-swallow: an Option; an account with no cache for these credentials starts empty
        state.serial=state.serial.wrapping_add(1);let serial=state.serial;state.active+=1;state.active_users.insert(user);
        state.entries.insert(user,Entry{identity:identity.into(),revision,serial,started:now,until:now,loading:true,ready:false,reason:None,cache:cache.clone()});
        Load::Start(Ticket{service:self.clone(),user,serial,finished:false},cache)
    }
    /// A page asked for this user's figures and they could not be published yet (loading::publish). A fill already
    /// running for it was its first attempt.
    pub fn want(&self,user:i64) {
        let mut state=self.state();
        let running=state.active_users.contains(&user);
        state.wanted.entry(user).or_insert(if running { ALLOWANCE-1 } else { ALLOWANCE });
    }
    /// They were published.
    pub fn served(&self,user:i64) { self.state().wanted.remove(&user); }
    /// The fills this waiting account may still start (`None`: it does not wait).
    pub fn allowance(&self,user:i64)->Option<u8> { self.state().wanted.get(&user).copied() }
    /// Who waits and has no fill running: what the end of any fill publishes for.
    pub fn waiting_idle(&self)->Vec<i64> {
        let state=self.state();
        state.wanted.keys().copied().filter(|user|!state.active_users.contains(user)).collect()
    }
    /// The engine wrote an order of this bot: O(1), and never waits, so marking goes on during a publication. At most
    /// one mark per bot is kept, however many orders.
    pub fn mark(&self,bot:i64) { if self.state().marked.insert(bot) { self.1.notify_one(); } }
    /// The bots marked since the last call.
    pub fn take_marked(&self)->BTreeSet<i64> { std::mem::take(&mut self.state().marked) }
    pub fn marked(&self)->usize { self.state().marked.len() }
    /// Until a bot or an account is marked (at once if one was marked since the last wait).
    pub async fn marked_wait(&self) { self.1.notified().await }
    /// A publication of this account failed at the end of a fill (a transient database error): the `figures` service
    /// tries it again, with the same backoff as a marked bot's. One entry per account, however many failures.
    pub fn mark_owner(&self,user:i64) { if self.state().owners.insert(user) { self.1.notify_one(); } }
    /// The accounts marked since the last call.
    pub fn take_owners(&self)->BTreeSet<i64> { std::mem::take(&mut self.state().owners) }
    pub fn has_owners(&self)->bool { !self.state().owners.is_empty() }
    /// The serial of the figures kept for this account (0: none), taken as a publication begins (`record`).
    pub fn settled(&self,user:i64)->u64 { self.state().entries.get(&user).map_or(0,|e|e.serial) }
    /// Whether these are the figures kept for this account and still fresh: ready, of this `serial` with no fill begun
    /// since, within their lifetime, for these credentials (`identity`) and this database `revision`, as a page load
    /// would find them `Ready`.
    fn fresh(state:&State,user:i64,serial:u64,identity:&str,revision:u64,now:i64)->bool {
        state.entries.get(&user).is_some_and(|e|!e.loading&&e.ready&&e.serial==serial&&crate::engine::model::identity_current_for(&e.identity,identity).is_fresh()&&e.revision==revision&&e.until>now)
    }
    /// A publication about to be delivered: `true` if the figures it rendered (`serial`, from `settled`) are still fresh,
    /// and then it is kept as this account's latest, so that a connection that subscribes again is sent it (`cable`: a
    /// publication a full mailbox dropped). `false` otherwise: it must not be delivered, and the account's latest is
    /// forgotten. A publication past the mailbox's bounds is kept as incomplete, which `latest` never sends.
    pub fn record(&self,user:i64,serial:u64,identity:&str,revision:u64,now:i64,streams:&[(String,String)])->bool {
        self.record_with(user,serial,identity,revision,now,streams,FxDeps::default(),|_|true)
    }
    /// `record`, for figures that read held conversions (`fx`): accepted only if `holds` says those conversions still hold
    /// as it is accepted, and a replay checks them again (`latest_with`).
    #[allow(clippy::too_many_arguments)]
    pub fn record_with(&self,user:i64,serial:u64,identity:&str,revision:u64,now:i64,streams:&[(String,String)],fx:FxDeps,holds:impl FnOnce(&FxDeps)->bool)->bool {
        use crate::web::cable::{MAILBOX_BYTES,MAILBOX_ENTRIES};
        let mut state=self.state();
        if !Self::fresh(&state,user,serial,identity,revision,now) || !holds(&fx) { state.latest.remove(&user); return false; }
        let (mut kept,mut bytes)=(Vec::new(),0);
        for (stream,html) in streams {
            if kept.len()>=MAILBOX_ENTRIES || bytes+html.len()>MAILBOX_BYTES { break; }
            bytes+=html.len();
            kept.push((Arc::from(stream.as_str()),Arc::from(html.as_str())));
        }
        let complete=kept.len()==streams.len();
        state.latest.insert(user,(serial,complete,kept,fx));
        true
    }
    /// This account's latest payloads on `stream`, only if all of that publication was kept and its figures are still
    /// fresh. `None` otherwise (incomplete, expired, written since, other credentials, evicted or never kept): nothing
    /// stale or partial is sent, and the caller asks for a publication instead (`loading::resubscribed`).
    pub fn latest(&self,user:i64,identity:&str,revision:u64,now:i64,stream:&str)->Option<Vec<Arc<str>>> {
        self.latest_with(user,identity,revision,now,stream).map(|(kept,_)|kept)
    }
    /// `latest`, with the held conversions its figures read, for the caller to check before replaying them.
    pub fn latest_with(&self,user:i64,identity:&str,revision:u64,now:i64,stream:&str)->Option<(Vec<Arc<str>>,FxDeps)> {
        let mut state=self.state();
        if state.entries.get(&user).is_some_and(|entry|!crate::engine::model::identity_current_for(&entry.identity,identity).is_fresh()) {
            state.entries.remove(&user);state.latest.remove(&user);return None;
        }
        let (serial,complete,kept,fx)=state.latest.get(&user)?;
        if !*complete || !Self::fresh(&state,user,*serial,identity,revision,now) { return None; }
        Some((kept.iter().filter(|(on,_)|&**on==stream).map(|(_,html)|html.clone()).collect(),fx.clone()))
    }
}
impl Ticket {
    /// `now`: when the fill ended. A failure is kept for a minute from then, however long the fill ran.
    pub fn finish(self,cache:Cache,ready:bool,now:i64) { self.finish_with_reason(cache,ready,None,now); }
    pub fn finish_with_reason(self,cache:Cache,ready:bool,reason:Option<&'static str>,now:i64) { self.finish_until(cache,ready,reason,now,None); }
    /// `deadline`: when ready figures stop being true before the next five-minute cut (a wash-sale lock's end).
    pub fn finish_until(mut self,cache:Cache,ready:bool,reason:Option<&'static str>,now:i64,deadline:Option<i64>) { self.store(cache,ready,reason,Some(now),deadline);self.finished=true; }
    fn store(&self,cache:Cache,ready:bool,reason:Option<&'static str>,ended:Option<i64>,deadline:Option<i64>) {
        let mut state=self.service.state();
        state.active=state.active.saturating_sub(1);state.active_users.remove(&self.user);
        if let Some(entry)=state.entries.get_mut(&self.user).filter(|e|e.serial==self.serial) {
            entry.loading=false;entry.ready=ready;entry.reason=if ready{None}else{reason};entry.cache=cache;
            // Ready figures last until the next five-minute cut (UTC midnight is one), or a lock's end before it.
            entry.until=if ready { entry.started.saturating_add(300-entry.started.rem_euclid(300)).min(deadline.unwrap_or(i64::MAX)) } else { ended.unwrap_or(entry.started).max(entry.started).saturating_add(60) };
        }
        // What arrived while this fill ran is due now.
        if let Some(mut work)=state.parked.remove(&self.user) { work.not_before=i64::MIN; put(&mut state.pending,self.user,work); }
        drop(state);
        self.service.1.notify_one();
    }
}
impl Drop for Ticket { fn drop(&mut self) { if !self.finished { self.store(Cache::default(),false,None,None,None); } } }

#[cfg(test)]
mod r4_service_tests {
    use super::*;
    fn identity(n:u8)->String{serde_json::json!([1,{"key_id":1,"ciphertext_digest":hex::encode([n;32])}]).to_string()}
    fn ready()->(Service,String,u64){
        let service=Service::default();let a=identity(1);
        let Load::Start(ticket,cache)=service.begin(1,&a,7,1) else{panic!("control fill")};ticket.finish(cache,true,1);
        let serial=service.settled(1);(service,a,serial)
    }
    #[test]
    fn r4_service_begin_and_loading_evict_stale_values_without_parallel_account_fills(){
        let (service,a,serial)=ready();assert!(service.record(1,serial,&a,7,2,&[("s".into(),"A".into())]));
        assert!(matches!(service.begin(1,&a,7,2),Load::Ready(..)));
        assert!(matches!(service.begin(1,&identity(2),7,2),Load::Start(..)),"R4 B begins a new fill, never A ready state");
        assert!(!service.state().latest.contains_key(&1),"R4 beginning B drops A's latest payload");
        let service=Service::default();let Load::Start(ticket,_)=service.begin(1,&a,7,1) else{panic!("control")};
        assert!(matches!(service.begin(1,&identity(2),7,2),Load::Cold),"R4 only one network fill per account");
        assert!(!service.state().entries.contains_key(&1),"R4 stale in-flight value is physically dropped");
        ticket.finish(Cache::default(),true,2);
        assert!(matches!(service.begin(1,&identity(2),7,3),Load::Start(..)));
    }
    #[test]
    fn r4_service_publication_checks_the_producer(){
        let (service,a,serial)=ready();let streams=vec![("s".into(),"A".into())];
        assert!(service.record(1,serial,&a,7,2,&streams));
        assert!(!service.record(1,serial,&identity(2),7,2,&streams),"R4 A publication cannot be accepted as B");
    }
    #[test]
    fn r4_service_latest_evicts_a_payload_after_rotation(){
        let (service,a,serial)=ready();assert!(service.record(1,serial,&a,7,2,&[("s".into(),"A".into())]));
        assert!(service.latest(1,&a,7,2,"s").is_some());
        assert!(service.latest(1,&identity(2),7,2,"s").is_none(),"R4 A latest publication cannot replay for B");
        assert!(!service.state().entries.contains_key(&1)&&!service.state().latest.contains_key(&1),"R4 stale publication entry is dropped");
    }
}
