//! At most two fills, no waiting queue, and no decimal crosses this boundary.
use crate::figures::page_market::Cache;
use std::collections::BTreeMap;
use std::sync::{Arc,Mutex,MutexGuard};

const ACCOUNTS:usize=8;
#[derive(Default,Clone)]
pub struct Service(Arc<Mutex<State>>);
#[derive(Default)]
struct State { active:usize, serial:u64, entries:BTreeMap<i64,Entry> }
struct Entry { identity:String,revision:u64,serial:u64,started:i64,until:i64,loading:bool,ready:bool,cache:Cache }
pub enum Load { Cold, Failed, Ready(Cache,i64), Start(Ticket,Cache) }
pub struct Ticket { service:Service,user:i64,serial:u64,finished:bool }
impl Service {
    fn state(&self)->MutexGuard<'_,State> { self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) }
    pub fn begin(&self,user:i64,identity:&str,revision:u64,now:i64)->Load {
        let mut state=self.state();
        if let Some(entry)=state.entries.get(&user).filter(|e|e.identity==identity&&e.revision==revision) {
            if entry.loading { return Load::Cold; }
            if entry.until>now { return if entry.ready { Load::Ready(entry.cache.clone(),entry.started) } else { Load::Failed }; }
        }
        if state.active>=2 { return Load::Failed; }
        if state.entries.len()>=ACCOUNTS && !state.entries.contains_key(&user) {
            let victim=state.entries.iter().filter(|(_,e)|!e.loading).min_by_key(|(_,e)|e.started).map(|(id,_)|*id);
            if let Some(id)=victim { state.entries.remove(&id); } else { return Load::Failed; }
        }
        let cache=state.entries.get(&user).filter(|e|e.identity==identity).map(|e|e.cache.clone()).unwrap_or_default();
        state.serial=state.serial.wrapping_add(1);let serial=state.serial;state.active+=1;
        state.entries.insert(user,Entry{identity:identity.into(),revision,serial,started:now,until:now,loading:true,ready:false,cache:cache.clone()});
        Load::Start(Ticket{service:self.clone(),user,serial,finished:false},cache)
    }
}
impl Ticket {
    pub fn finish(mut self,cache:Cache,ready:bool) { self.store(cache,ready);self.finished=true; }
    fn store(&self,cache:Cache,ready:bool) {
        let mut state=self.service.state();
        state.active=state.active.saturating_sub(1);
        if let Some(entry)=state.entries.get_mut(&self.user).filter(|e|e.serial==self.serial) {
            entry.loading=false;entry.ready=ready;entry.cache=cache;
            entry.until=if ready { entry.started.saturating_add(300-entry.started.rem_euclid(300)) } else { entry.started.saturating_add(60) };
        }
    }
}
impl Drop for Ticket { fn drop(&mut self) { if !self.finished { self.store(Cache::default(),false); } } }
