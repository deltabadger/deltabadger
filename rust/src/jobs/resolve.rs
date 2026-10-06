//! Construction of every registered job type; scheduler handles late scope registration.
use super::{Job,Resolver,data_api::DataApi};
use crate::{sync::jobs::{self,Connect},venue::http::Transport,tracker};
use std::rc::Rc;
pub fn all<C:Connect+Clone+'static,T:Transport+'static>(venues:C,api:Rc<Option<DataApi<T>>>,wall:tracker::jobs::Wall)->Resolver{
    Box::new(move|name,scope|{
        if scope.is_none(){
            return super::reference::shared_jobs(api.clone()).into_iter().find(|job|job.spec().name==name).ok_or_else(||"unknown unscoped job".into());
        }
        let id=scope.and_then(|s|s.parse::<i64>().ok()).filter(|id|*id>0).ok_or("invalid job scope")?;
        let job:Box<dyn Job>=match name{
            jobs::LEDGER_SYNC=>Box::new(jobs::LedgerSync::new(venues.clone(),id)),
            jobs::BALANCE_SYNC=>Box::new(jobs::BalanceSync::new(venues.clone(),api.clone(),id)),
            _=>tracker::jobs::resolve(name,id,&venues,api.clone(),wall.clone()).ok_or("unknown scoped job")?,
        };
        Ok(job)
    })
}
#[cfg(test)]
mod tests{
    #[test]
    fn resolves_every_builtin_job_family(){
        use super::*;
        use crate::venue::{alpaca::LiveFactory,http::ScriptedTransport};
        let api:Rc<Option<DataApi<ScriptedTransport>>>=Rc::new(None);
        let resolve=all(LiveFactory::new(),api,tracker::jobs::system_wall());
        for spec in crate::jobs::reference::specs(){assert_eq!(resolve(spec.name,None).unwrap().spec(),spec);}
        for name in [jobs::LEDGER_SYNC,jobs::BALANCE_SYNC,tracker::jobs::TRACKER_LEDGER,tracker::jobs::PORTFOLIO_BACKFILL]{
            let spec=resolve(name,Some("42")).unwrap().spec();
            assert_eq!(spec.name,name);assert_eq!(spec.scope.as_deref(),Some("42"));
        }
        assert!(resolve("unknown",None).is_err());assert!(resolve(jobs::LEDGER_SYNC,Some("9223372036854775808")).is_err());
    }
}
