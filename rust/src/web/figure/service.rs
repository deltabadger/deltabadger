//! At most two fills, one per account, no waiting queue, and no decimal crosses this boundary. Who waits for a
//! publication (`want`) is kept here with that account's own allowance of fills, so that a fill that is turned away
//! or overtaken leaves its demand behind, and waiting behind another account never spends it.
use crate::figures::page_market::Cache;
use std::collections::{BTreeMap,BTreeSet};
use std::sync::{Arc,Mutex,MutexGuard};
use tokio::sync::Notify;

const ACCOUNTS:usize=8;
/// Fills one waiting account may start before its wait ends in no value: the first, and two overtaken by writes.
pub const ALLOWANCE:u8=3;
#[derive(Default,Clone)]
pub struct Service(Arc<Mutex<State>>,Arc<Notify>);
#[derive(Default)]
struct State { active:usize, serial:u64, entries:BTreeMap<i64,Entry>, wanted:BTreeMap<i64,u8>, marked:BTreeSet<i64>, owners:BTreeSet<i64>, latest:BTreeMap<i64,Latest> }
/// An account's last publication: the serial of the figures it rendered, whether all of it was kept, and (stream,
/// payload) one per target as a publication carries them, within the mailbox's bounds.
type Latest=(u64,bool,Vec<(Arc<str>,Arc<str>)>);
struct Entry { identity:String,revision:u64,serial:u64,started:i64,until:i64,loading:bool,ready:bool,cache:Cache }
/// `Busy`: both fills are taken by other accounts. `Failed` also when a fill is needed and the waiting account has no
/// allowance left.
pub enum Load { Cold, Failed, Busy, Ready(Cache,i64), Start(Ticket,Cache) }
pub struct Ticket { service:Service,user:i64,serial:u64,finished:bool }
impl Service {
    fn state(&self)->MutexGuard<'_,State> { self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) }
    pub fn begin(&self,user:i64,identity:&str,revision:u64,now:i64)->Load {
        let mut state=self.state();
        // One fill per account, whatever revision it was started for: its end starts the next one if a page still waits.
        if state.entries.get(&user).is_some_and(|e|e.loading) { return Load::Cold; }
        if let Some(entry)=state.entries.get(&user).filter(|e|e.identity==identity&&e.revision==revision) {
            if entry.until>now { return if entry.ready { Load::Ready(entry.cache.clone(),entry.started) } else { Load::Failed }; }
        }
        if state.wanted.get(&user)==Some(&0) { return Load::Failed; }
        // Waiting for capacity spends nothing.
        if state.active>=2 { return Load::Busy; }
        if state.entries.len()>=ACCOUNTS && !state.entries.contains_key(&user) {
            let victim=state.entries.iter().filter(|(_,e)|!e.loading).min_by_key(|(_,e)|e.started).map(|(id,_)|*id);
            if let Some(id)=victim { state.entries.remove(&id);state.latest.remove(&id); } else { return Load::Busy; }
        }
        if let Some(allowance)=state.wanted.get_mut(&user) { *allowance-=1; }
        let cache=state.entries.get(&user).filter(|e|e.identity==identity).map(|e|e.cache.clone()).unwrap_or_default(); // allow-swallow: an Option; an account with no cache for these credentials starts empty
        state.serial=state.serial.wrapping_add(1);let serial=state.serial;state.active+=1;
        state.entries.insert(user,Entry{identity:identity.into(),revision,serial,started:now,until:now,loading:true,ready:false,cache:cache.clone()});
        Load::Start(Ticket{service:self.clone(),user,serial,finished:false},cache)
    }
    /// A page asked for this user's figures and they could not be published yet (loading::publish). A fill already
    /// running for it was its first attempt.
    pub fn want(&self,user:i64) {
        let mut state=self.state();
        let running=state.entries.get(&user).is_some_and(|e|e.loading);
        state.wanted.entry(user).or_insert(if running { ALLOWANCE-1 } else { ALLOWANCE });
    }
    /// They were published.
    pub fn served(&self,user:i64) { self.state().wanted.remove(&user); }
    /// The fills this waiting account may still start (`None`: it does not wait).
    pub fn allowance(&self,user:i64)->Option<u8> { self.state().wanted.get(&user).copied() }
    /// Who waits and has no fill running: what the end of any fill publishes for.
    pub fn waiting_idle(&self)->Vec<i64> {
        let state=self.state();
        state.wanted.keys().copied().filter(|user|!state.entries.get(user).is_some_and(|e|e.loading)).collect()
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
    /// The serial of the figures kept for this account (0: none), taken as a publication begins (`record`).
    pub fn settled(&self,user:i64)->u64 { self.state().entries.get(&user).map_or(0,|e|e.serial) }
    /// Whether these are the figures kept for this account and still fresh: ready, of this `serial` with no fill begun
    /// since, within their lifetime, for these credentials (`identity`) and this database `revision`, as a page load
    /// would find them `Ready`.
    fn fresh(state:&State,user:i64,serial:u64,identity:&str,revision:u64,now:i64)->bool {
        state.entries.get(&user).is_some_and(|e|!e.loading&&e.ready&&e.serial==serial&&e.identity==identity&&e.revision==revision&&e.until>now)
    }
    /// A publication about to be delivered: `true` if the figures it rendered (`serial`, from `settled`) are still fresh,
    /// and then it is kept as this account's latest, so that a connection that subscribes again is sent it (`cable`: a
    /// publication a full mailbox dropped). `false` otherwise: it must not be delivered, and the account's latest is
    /// forgotten. A publication past the mailbox's bounds is kept as incomplete, which `latest` never sends.
    pub fn record(&self,user:i64,serial:u64,identity:&str,revision:u64,now:i64,streams:&[(String,String)])->bool {
        use crate::web::cable::{MAILBOX_BYTES,MAILBOX_ENTRIES};
        let mut state=self.state();
        if !Self::fresh(&state,user,serial,identity,revision,now) { state.latest.remove(&user); return false; }
        let (mut kept,mut bytes)=(Vec::new(),0);
        for (stream,html) in streams {
            if kept.len()>=MAILBOX_ENTRIES || bytes+html.len()>MAILBOX_BYTES { break; }
            bytes+=html.len();
            kept.push((Arc::from(stream.as_str()),Arc::from(html.as_str())));
        }
        let complete=kept.len()==streams.len();
        state.latest.insert(user,(serial,complete,kept));
        true
    }
    /// This account's latest payloads on `stream`, only if all of that publication was kept and its figures are still
    /// fresh. `None` otherwise (incomplete, expired, written since, other credentials, evicted or never kept): nothing
    /// stale or partial is sent, and the caller asks for a publication instead (`loading::resubscribed`).
    pub fn latest(&self,user:i64,identity:&str,revision:u64,now:i64,stream:&str)->Option<Vec<Arc<str>>> {
        let state=self.state();
        let (serial,complete,kept)=state.latest.get(&user)?;
        if !*complete || !Self::fresh(&state,user,*serial,identity,revision,now) { return None; }
        Some(kept.iter().filter(|(on,_)|&**on==stream).map(|(_,html)|html.clone()).collect())
    }
}
impl Ticket {
    /// `now`: when the fill ended. A failure is kept for a minute from then, however long the fill ran.
    pub fn finish(mut self,cache:Cache,ready:bool,now:i64) { self.store(cache,ready,Some(now));self.finished=true; }
    fn store(&self,cache:Cache,ready:bool,ended:Option<i64>) {
        let mut state=self.service.state();
        state.active=state.active.saturating_sub(1);
        if let Some(entry)=state.entries.get_mut(&self.user).filter(|e|e.serial==self.serial) {
            entry.loading=false;entry.ready=ready;entry.cache=cache;
            entry.until=if ready { entry.started.saturating_add(300-entry.started.rem_euclid(300)) } else { ended.unwrap_or(entry.started).max(entry.started).saturating_add(60) };
        }
    }
}
impl Drop for Ticket { fn drop(&mut self) { if !self.finished { self.store(Cache::default(),false,None); } } }
