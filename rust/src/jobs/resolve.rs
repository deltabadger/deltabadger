//! Construction of every registered job type; scheduler handles late scope registration.
use super::{Job,Resolver,data_api::{DataApi,RateCache},market::Feed};
use crate::{sync::jobs::{self,Connect},venue::http::Transport,tracker};
use std::rc::Rc;
use std::sync::Arc;
pub fn all<C:Connect+Clone+'static,T:Transport+'static>(venues:C,api:Rc<Option<DataApi<T>>>,wall:tracker::jobs::Wall)->Resolver{
    let prices:Rc<dyn crate::jobs::data_api::PriceSource>=api.clone();
    all_with(venues,api,Rc::new(None),prices,wall)
}
/// `all`, the tracker and tax jobs asking `feed` (jobs::market): the reference and balance jobs keep `api`.
/// `prices`: the balance sync's (sync::balances::MarketPrices on the scheduler: the run's feed and Rails' CoinGecko fallback).
pub fn all_with<C:Connect+Clone+'static,T:Transport+'static>(venues:C,api:Rc<Option<DataApi<T>>>,feed:Rc<Option<Feed<T>>>,prices:Rc<dyn crate::jobs::data_api::PriceSource>,wall:tracker::jobs::Wall)->Resolver{
    Box::new(move|name,scope|{
        if scope.is_none(){
            // AppPaths.tmp is the process's (the binary's env is the process env).
            if name==crate::tax::job::JOB {return Ok(crate::tax::job::job(feed.clone(),venues.clone(),crate::tax::job::tmp_dir(&|k|std::env::var(k).ok())));}
            return super::reference::shared_jobs(api.clone()).into_iter().find(|job|job.spec().name==name).ok_or_else(||"unknown unscoped job".into());
        }
        let id=scope.and_then(|s|s.parse::<i64>().ok()).filter(|id|*id>0).ok_or("invalid job scope")?;
        let job:Box<dyn Job>=match name{
            jobs::API_KEY_VALIDATOR=>Box::new(jobs::Validator{venues:venues.clone(),key_id:id}),
            jobs::LEDGER_SYNC=>Box::new(jobs::LedgerSync::new(venues.clone(),id)),
            jobs::BALANCE_SYNC=>Box::new(jobs::BalanceSync::new(venues.clone(),prices.clone(),id)),
            super::reference::ASSET_DATA=>Box::new(super::reference::AssetData{cg:super::coingecko::live_clients(),asset:id}),
            _=>tracker::jobs::resolve(name,id,&venues,feed.clone(),wall.clone()).ok_or("unknown scoped job")?,
        };
        Ok(job)
    })
}
/// Live scheduler jobs resolve the market-data configuration at the start of each run. Keep the
/// original job for its spec and event subscriptions; construct its consumer with the current client.
/// This also wraps late scopes, so a claim applies to balance and tracker jobs created after startup.
/// `rates`: the engine's one fiat-rate cache (Utilities::Currency's Rails.cache entries), shared by every run.
pub fn configured(job: Box<dyn Job>, env: &dyn Fn(&str) -> Option<String>, rates: Arc<RateCache>) -> Box<dyn Job> {
    configured_with(job, env, rates, crate::venue::alpaca::LiveFactory::new())
}

/// `configured`, reaching the venues through `venues`.
pub fn configured_with<C: Connect + Clone + 'static>(job: Box<dyn Job>, env: &dyn Fn(&str) -> Option<String>, rates: Arc<RateCache>, venues: C) -> Box<dyn Job> {
    configured_feeds(job, env, rates, venues, Rc::new(Feed::live) as Feeds<super::market::Live>)
}

/// What a run's feed is made of: the settings read at its start, and the engine's rate cache.
pub type Feeds<T> = Rc<dyn Fn(&crate::market_data::Settings, Arc<RateCache>) -> Option<Feed<T>>>;

/// `configured_with`, the run's clients made by `feeds` (tests: scripted transports).
pub fn configured_feeds<C: Connect + Clone + 'static, T: Transport + 'static>(job: Box<dyn Job>, env: &dyn Fn(&str) -> Option<String>, rates: Arc<RateCache>, venues: C, feeds: Feeds<T>) -> Box<dyn Job> {
    let name = job.spec().name;
    let uses_data = super::reference::specs().iter().any(|s| s.name == name && name != super::reference::PRUNE)
        || matches!(name, jobs::BALANCE_SYNC | tracker::jobs::TRACKER_LEDGER | tracker::jobs::PORTFOLIO_BACKFILL | crate::tax::job::JOB);
    if !uses_data { return job; }
    let env = ["MARKET_DATA_URL", "MARKET_DATA_TOKEN", "COINGECKO_API_KEY"].into_iter().filter_map(|name| env(name).map(|value| (name, value))).collect();
    Box::new(Configured { job, env, rates, venues, feeds })
}

pub fn configured_resolver(resolve: Resolver, env: &dyn Fn(&str) -> Option<String>, rates: Arc<RateCache>) -> Resolver {
    let env: Vec<_> = ["MARKET_DATA_URL", "MARKET_DATA_TOKEN", "COINGECKO_API_KEY"].into_iter().filter_map(|name| env(name).map(|value| (name, value))).collect();
    Box::new(move |name, scope| {
        let job = resolve(name, scope)?;
        Ok(configured(job, &|name| env.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone()), rates.clone()))
    })
}

struct Configured<C: Connect, T: Transport> {
    job: Box<dyn Job>,
    env: Vec<(&'static str, String)>,
    rates: Arc<RateCache>,
    venues: C,
    feeds: Feeds<T>,
}

impl<C: Connect + Clone + 'static, T: Transport + 'static> Job for Configured<C, T> {
    fn spec(&self) -> super::Spec { self.job.spec() }
    fn wants(&self, event: &crate::engine::events::EngineEvent) -> bool { self.job.wants(event) }
    fn run<'a>(&'a self, cx: super::Cx<'a>, wakes: Vec<super::Wake>) -> super::JobFuture<'a> {
        Box::pin(async move { self.run_attributed(cx, wakes).await.value })
    }
    fn run_attributed<'a>(&'a self, cx: super::Cx<'a>, wakes: Vec<super::Wake>) -> super::AttributedJobFuture<'a> {
        Box::pin(async move {
            let env = self.env.clone();
            let settings = cx.db.run(move |c, cipher| {
                let env = |name: &str| env.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone());
                super::data_api::config(&env, c, cipher)?;
                crate::market_data::read(c, cipher, &crate::market_data::Env::read(&env))
            }).await;
            let settings = match settings {
                Ok(settings) => settings,
                Err(e) => return crate::engine::model::Produced::new(super::Outcome::Failed(e), None),
            };
            // One snapshot of the configuration per run: a switch takes effect at the next run (PLAN-A17 A17-3 item 1).
            let api = Rc::new((self.feeds)(&settings, self.rates.clone()).and_then(Feed::into_data_api));
            let feed = Rc::new((self.feeds)(&settings, self.rates.clone()));
            // The balance sync's prices: the run's feed, and a CoinGecko client for Rails' fallback made by the same factory.
            let fallback = |key: &str| {
                let only = crate::market_data::Settings { provider: crate::market_data::Provider::CoinGecko { key: Some(key.into()) }, ..settings.clone() };
                match (self.feeds)(&only, self.rates.clone()) { Some(Feed::CoinGecko(cg)) => cg, _ => unreachable!("a CoinGecko provider makes a CoinGecko feed") }
            };
            let prices = Rc::new(crate::sync::balances::MarketPrices::new(&settings, (self.feeds)(&settings, self.rates.clone()), fallback, self.rates.clone()));
            let spec = self.spec();
            let resolve = all_with(self.venues.clone(), api, feed, prices, tracker::jobs::system_wall());
            match resolve(spec.name, spec.scope.as_deref()) {
                Ok(job) => job.run_attributed(cx, wakes).await,
                Err(e) => crate::engine::model::Produced::new(super::Outcome::Failed(e), None),
            }
        })
    }
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
        for name in [jobs::LEDGER_SYNC,jobs::BALANCE_SYNC,tracker::jobs::TRACKER_LEDGER,tracker::jobs::PORTFOLIO_BACKFILL,crate::jobs::reference::ASSET_DATA]{
            let spec=resolve(name,Some("42")).unwrap().spec();
            assert_eq!(spec.name,name);assert_eq!(spec.scope.as_deref(),Some("42"));
        }
        assert!(resolve("unknown",None).is_err());assert!(resolve(jobs::LEDGER_SYNC,Some("9223372036854775808")).is_err());
    }
}
