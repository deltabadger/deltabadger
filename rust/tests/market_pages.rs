use deltabadger::figures::market::{MarketData, Venue};
use deltabadger::figures::page_market::{Cache, Reader};
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::json;
use std::cell::RefCell;

struct Wire { calls: RefCell<Vec<HttpRequest>>, body: String }
impl Transport for Wire {
    async fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.calls.borrow_mut().push(request.clone());
        Ok(HttpResponse { status: 200, body: self.body.clone() })
    }
}
fn venue() -> Venue { Venue { exchange_id: 1, exchange_type: "Exchanges::Alpaca".into() } }

#[tokio::test]
async fn prices_expire_and_missing_trades_are_not_zero() {
    let mut cache = Cache::default();
    let symbols = vec!["BBB".into(), "AAA".into()];
    let wire = Wire { calls: RefCell::default(), body: json!({"AAA":{"latestTrade":{"p":104.52}},"BBB":{}}).to_string() };
    let reader = Reader::new(&cache, 100);
    assert!(reader.prices(&venue(), &symbols).is_err());
    let demand = reader.demands();
    assert_eq!(demand.len(), 1);
    assert_eq!(demand[0].query, vec![("symbols", "AAA,BBB".into())]);
    cache.fill(&wire, demand, 100).await;
    let reader = Reader::new(&cache, 159);
    let prices = reader.prices(&venue(), &symbols).unwrap();
    assert_eq!(prices.len(), 1);
    assert_eq!(prices[0].0, "AAA");
    assert_eq!(prices[0].1.as_ref().unwrap().to_s_f(), "104.52");
    assert!(reader.demands().is_empty());
    assert!(Reader::new(&cache, 160).prices(&venue(), &symbols).is_err());
    assert_eq!(wire.calls.borrow().len(), 1);
}

#[tokio::test]
async fn crypto_empty_requests_failures_and_bad_unused_numbers() {
    let mut cache = Cache::default();
    let symbols = vec!["BTC/USD".into(), "ETH/USD".into(), "BAD/USD".into()];
    let wire = Wire { calls: RefCell::default(), body: json!({"trades":{"BTC/USD":{"p":0},"ETH/USD":{},"BAD/USD":{"p":"NaN"}}}).to_string() };
    assert!(Reader::new(&cache, 0).prices(&venue(), &[]).unwrap().is_empty());
    let reader = Reader::new(&cache, 0);
    let _ = reader.prices(&venue(), &symbols);
    cache.fill(&wire, reader.demands(), 0).await;
    let prices = Reader::new(&cache, 0).prices(&venue(), &symbols).unwrap();
    assert_eq!(prices.len(), 2);
    assert!(prices.iter().find(|p| p.0 == "BTC/USD").unwrap().1.as_ref().unwrap().is_zero());
    assert!(prices.iter().find(|p| p.0 == "BAD/USD").unwrap().1.is_err());
    assert_eq!(wire.calls.borrow()[0].base, "https://data.alpaca.markets");
    assert_eq!(wire.calls.borrow()[0].path, "/v1beta3/crypto/us/latest/trades");
}

fn ticker(crypto: bool) -> deltabadger::figures::db::Ticker {
    deltabadger::figures::db::Ticker { id: 1, ticker: if crypto { "BTC/USD" } else { "AAA" }.into(), base: "AAA".into(), base_asset_id: 1,
        quote_decimals: Some(2), base_category: Some(if crypto { "Cryptocurrency" } else { "Stock" }.into()), base_asset_exists: true }
}

#[tokio::test]
async fn closed_candles_overlap_and_rebuild_instead_of_splicing_a_split() {
    use deltabadger::figures::at::At;
    let mut cache = Cache::default();
    let at = |seconds: i64| At(seconds * 1_000_000_000);
    let bars = |price: f64| json!({"bars":[{"t":"1970-01-01T00:01:00Z","o":price},{"t":"1970-01-01T00:00:00Z","o":10},{"t":"1970-01-01T00:02:00Z","o":30}]}).to_string();
    let wire = Wire { calls: RefCell::default(), body: bars(20.0) };
    let reader = Reader::new(&cache, 120);
    assert!(reader.candles(&venue(), &ticker(false), at(0), 60, true).is_err());
    cache.fill(&wire, reader.demands(), 120).await;
    let reader = Reader::new(&cache, 179);
    let got = reader.candles(&venue(), &ticker(false), at(0), 60, true).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].0, at(0));
    assert!(reader.demands().is_empty());
    let reader = Reader::new(&cache, 180);
    assert!(reader.candles(&venue(), &ticker(false), at(0), 60, true).is_err());
    let demand = reader.demands();
    assert!(demand[0].query.contains(&("start", "1970-01-01T00:00:00Z".into())));
    // Cache.fill changes the wire start to the overlapping bar; a restatement then refetches the head.
    let changed = Wire { calls: RefCell::default(), body: bars(2.0) };
    cache.fill(&changed, demand, 180).await;
    assert_eq!(changed.calls.borrow().len(), 2);
    assert!(changed.calls.borrow()[0].query.contains(&("start", "1970-01-01T00:01:00Z".into())));
    assert!(changed.calls.borrow()[1].query.contains(&("start", "1970-01-01T00:00:00Z".into())));
    let got = Reader::new(&cache, 180).candles(&venue(), &ticker(false), at(0), 60, true).unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!(got[1].1.to_s_f(), "2.0");
}

#[test]
fn page_loads_coalesce_and_late_results_cannot_replace_rotated_credentials() {
    use deltabadger::web::figure::service::{Service, Load};
    let service = Service::default();
    let Load::Start(first, _) = service.begin(1,"key-a",1,100) else { panic!("first read starts fill") };
    assert!(matches!(service.begin(1,"key-a",1,100),Load::Cold));
    // One fill per account: new credentials, or a revision a write moved, wait for the running fill to end.
    assert!(matches!(service.begin(1,"key-b",2,100),Load::Cold));
    let Load::Start(other, _) = service.begin(2,"key-c",1,100) else { panic!("another account starts its own fill") };
    assert!(matches!(service.begin(3,"key-d",1,100),Load::Busy), "both fills are taken");
    first.finish(Cache::default(),true,100);
    let Load::Start(second, _) = service.begin(1,"key-b",2,100) else { panic!("new credentials start once the old fill ends") };
    second.finish(Cache::default(),true,100);
    other.finish(Cache::default(),true,100);
    assert!(matches!(service.begin(1,"key-b",2,299),Load::Ready(_,100)));
    assert!(matches!(service.begin(1,"key-b",2,300),Load::Start(_, _)));
    // Dropping a cancelled fill becomes a terminal state with bounded retry, not an eternal spinner.
    assert!(matches!(service.begin(1,"key-b",2,300),Load::Failed(_)));
}

#[test]
fn a_failure_lasts_a_minute_from_its_end_and_a_wait_outlives_a_full_house() {
    use deltabadger::web::figure::service::{Service, Load, ALLOWANCE};
    let service = Service::default();
    let Load::Start(slow, _) = service.begin(1,"k",1,0) else { panic!("starts") };
    slow.finish(Cache::default(),false,90); // failed at the 90-second timeout
    assert!(matches!(service.begin(1,"k",1,149),Load::Failed(_)), "kept for a minute from its end, not its start");
    assert!(matches!(service.begin(1,"k",1,150),Load::Start(_, _)));
    // Who waits is kept until published; an account whose fill runs is not idle.
    let service = Service::default();
    let Load::Start(running, _) = service.begin(1,"k",1,0) else { panic!("starts") };
    service.want(1);
    service.want(2);
    assert_eq!(service.waiting_idle(), vec![2]);
    running.finish(Cache::default(),true,0);
    assert_eq!(service.waiting_idle(), vec![1, 2]);
    service.served(1);
    assert_eq!(service.waiting_idle(), vec![2]);
    // A waiting account's own fills spend its allowance; with none left a fill is refused, and the wait ends.
    let service = Service::default();
    service.want(9);
    for revision in 1..=u64::from(ALLOWANCE) { // each overtaken by a write
        let Load::Start(attempt, _) = service.begin(9,"k",revision,0) else { panic!("an allowed attempt") };
        attempt.finish(Cache::default(),false,0);
    }
    assert_eq!(service.allowance(9), Some(0));
    assert!(matches!(service.begin(9,"k",u64::from(ALLOWANCE) + 1,0),Load::Failed(_)));
}

#[test]
fn waiting_behind_other_accounts_spends_no_allowance() {
    use deltabadger::web::figure::service::{Service, Load, ALLOWANCE};
    // A and B fill; C and D wait for capacity. A ends and C starts; C ends before B, and D still gets its own attempt.
    let service = Service::default();
    let Load::Start(a, _) = service.begin(1,"a",1,0) else { panic!("A starts") };
    let Load::Start(b, _) = service.begin(2,"b",1,0) else { panic!("B starts") };
    for user in [3, 4] {
        assert!(matches!(service.begin(user,"w",1,0),Load::Busy));
        service.want(user);
    }
    a.finish(Cache::default(),true,0);
    let Load::Start(c, _) = service.begin(3,"w",1,0) else { panic!("C gets its attempt") };
    assert!(matches!(service.begin(4,"w",1,0),Load::Busy));
    assert_eq!((service.allowance(3), service.allowance(4)), (Some(ALLOWANCE - 1), Some(ALLOWANCE)), "only C's own fill spent");
    c.finish(Cache::default(),true,0);
    assert!(matches!(service.begin(4,"w",1,0),Load::Start(_, _)), "D gets its attempt once capacity frees");
    b.finish(Cache::default(),true,0);
}

#[test]
fn an_accounts_latest_publication_is_sent_again_only_while_its_figures_are_fresh_and_whole() {
    use deltabadger::web::cable::MAILBOX_ENTRIES;
    use deltabadger::web::figure::service::{Service, Load};
    let stream = "user_1:bot_updates";
    let published = vec![(stream.to_string(), "<turbo-stream action=\"replace\" target=\"global-pnl\"><template>1</template></turbo-stream>".to_string())];
    let fresh = |service: &Service, now: i64| service.latest(1, "k", 1, now, stream);
    let service = Service::default();
    let Load::Start(fill, _) = service.begin(1,"k",1,0) else { panic!("starts") };
    fill.finish(Cache::default(),true,0);
    let serial = service.settled(1);
    assert!(service.record(1, serial, "k", 1, 0, &published), "fresh figures are delivered");
    assert_eq!(fresh(&service, 0).unwrap().iter().map(|html| html.to_string()).collect::<Vec<_>>(), vec![published[0].1.clone()]);
    assert_eq!(service.latest(1, "k", 1, 0, "elsewhere").map(|kept| kept.len()), Some(0), "only that stream's");
    assert!(service.latest(2, "k", 1, 0, stream).is_none(), "only that account's");
    // Not once the figures expired (the cache's five minutes), the database was written, or the credentials changed;
    // and a publication that finds them so is not delivered.
    assert!(fresh(&service, 299).is_some() && fresh(&service, 300).is_none(), "expired figures sent again");
    assert!(service.latest(1, "k", 2, 0, stream).is_none(), "figures of an older revision sent again");
    assert!(service.latest(1, "other", 1, 0, stream).is_none(), "figures of other credentials sent again");
    assert!(!service.record(1, serial, "k", 1, 300, &published), "expired figures delivered");
    assert!(service.record(1, serial, "k", 1, 0, &published));
    // Nor once the account was evicted by eight others.
    for other in 2..=9 {
        let Load::Start(fill, _) = service.begin(other,"o",1,0) else { panic!("account {other} starts") };
        fill.finish(Cache::default(),true,0);
    }
    assert!(fresh(&service, 0).is_none(), "an evicted account's figures sent again");
    // A write starts the next fill: what was published is no longer the account's figures, during the fill or after it.
    let service = Service::default();
    let Load::Start(fill, _) = service.begin(1,"k",1,0) else { panic!("starts") };
    fill.finish(Cache::default(),true,0);
    let serial = service.settled(1);
    assert!(service.record(1, serial, "k", 1, 0, &published));
    let Load::Start(next, _) = service.begin(1,"k",2,0) else { panic!("the write's fill starts") };
    assert!(service.latest(1, "k", 2, 0, stream).is_none(), "sent again while a newer fill runs");
    next.finish(Cache::default(),true,0);
    assert!(service.latest(1, "k", 2, 0, stream).is_none(), "sent again after a newer fill");
    // A publication rendered before that fill began is neither delivered nor kept.
    assert!(!service.record(1, serial, "k", 2, 0, &published), "a publication of older figures delivered");
    assert!(service.latest(1, "k", 2, 0, stream).is_none(), "kept a publication of older figures");
    // An account past the mailbox's bounds: 512 bots publish 1537 payloads (a tile each on the account's stream, metrics
    // and a chart on each bot's, then the headline), and only the headline is left out; with 513 the last bot's whole
    // stream is. Either publication is delivered, but never sent again, whole stream or not.
    for bots in [512, 513] {
        let mut streams = Vec::new();
        for bot in 0..bots {
            streams.push((stream.to_string(), format!("<turbo-stream action=\"replace\" target=\"tile_{bot}\"><template></template></turbo-stream>")));
            for target in ["metrics", "chart"] { streams.push((format!("bot_{bot}:bot_updates"), format!("<turbo-stream action=\"replace\" target=\"{target}\"><template></template></turbo-stream>"))); }
        }
        streams.push((stream.to_string(), published[0].1.clone()));
        assert!(streams.len() > MAILBOX_ENTRIES);
        let service = Service::default();
        let Load::Start(fill, _) = service.begin(1,"k",1,0) else { panic!("starts") };
        fill.finish(Cache::default(),true,0);
        assert!(service.record(1, service.settled(1), "k", 1, 0, &streams), "{bots} bots: delivered");
        for on in [stream.to_string(), "bot_0:bot_updates".to_string(), format!("bot_{}:bot_updates", bots - 1)] {
            assert!(service.latest(1, "k", 1, 0, &on).is_none(), "{bots} bots: {on} sent again from an incomplete publication");
        }
    }
}

#[tokio::test]
async fn failed_refresh_and_hostile_bare_numbers_never_become_prices() {
    struct Reply(Result<HttpResponse,TransportError>);
    impl Transport for Reply {
        async fn send(&self,_:&HttpRequest)->Result<HttpResponse,TransportError>{self.0.clone()}
    }
    for reply in [
        Err(TransportError::NotSent("secret echo".into())),Err(TransportError::Permanent("secret echo".into())),
        Ok(HttpResponse{status:429,body:"secret echo".into()}),Ok(HttpResponse{status:200,body:"not JSON".into()}),
        Ok(HttpResponse{status:200,body:r#"{"AAA":{"latestTrade":{"p":1e-350}}}"#.into()}),
    ] {
        let mut cache=Cache::default();
        let symbols=vec!["AAA".into()];
        let reader=Reader::new(&cache,0);let _=reader.prices(&venue(),&symbols);
        cache.fill(&Reply(reply),reader.demands(),0).await;
        let reader=Reader::new(&cache,0);
        let error=reader.prices(&venue(),&symbols).unwrap_err();
        assert!(reader.failed());
        assert!(!format!("{error:?}").contains("secret echo"));
        assert!(reader.demands().is_empty());
        assert!(Reader::new(&cache,60).prices(&venue(),&symbols).is_err());
    }
}

#[test]
fn the_rails_sources_of_the_page_figures_are_pinned() {
    use sha2::{Digest,Sha256};
    let files:serde_json::Value=serde_json::from_str(include_str!("fixtures/page_figure_sources.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (file,hash) in files.as_object().unwrap(){
        assert_eq!(hex::encode(Sha256::digest(std::fs::read(root.join(file)).unwrap())),hash.as_str().unwrap(),"{file}: record the page parity grid after checking its port");
    }
}

struct Reply(Result<HttpResponse, TransportError>);
impl Transport for Reply {
    async fn send(&self, _: &HttpRequest) -> Result<HttpResponse, TransportError> { self.0.clone() }
}

#[tokio::test]
async fn candle_failures_mark_the_reader_failed_without_pending_demands() {
    use deltabadger::figures::at::At;
    for reply in [
        Ok(HttpResponse { status: 503, body: "secret echo".into() }),
        Err(TransportError::NotSent("connect timeout secret echo".into())),
        Err(TransportError::MaybeSent("read timeout secret echo".into())),
        Err(TransportError::Permanent("secret echo".into())),
        Ok(HttpResponse { status: 200, body: "unreadable secret echo".into() }),
    ] {
        for crypto in [false, true] {
            let mut cache = Cache::default();
            let reader = Reader::new(&cache, 120);
            assert!(reader.candles(&venue(), &ticker(crypto), At(0), 60, true).is_err());
            cache.fill(&Reply(reply.clone()), reader.demands(), 120).await;
            let reader = Reader::new(&cache, 120);
            let error = reader.candles(&venue(), &ticker(crypto), At(0), 60, true).unwrap_err();
            assert!(reader.failed(), "candle failure escaped publication guard: {error:?}");
            assert!(reader.demands().is_empty());
            assert!(!format!("{error:?}").contains("secret echo"));
        }
    }
}

#[tokio::test]
async fn closed_candle_prices_must_be_present_and_readable_but_may_be_zero() {
    use deltabadger::figures::at::At;
    for price in [None, Some(json!(null)), Some(json!("NaN")), Some(json!("Infinity")),
                  Some(json!("12garbage")), Some(json!("")), Some(json!(true)), Some(json!({})),
                  Some(json!(0)), Some(json!("12.5"))] {
        for crypto in [false, true] {
            let mut bar = json!({"t":"1970-01-01T00:00:00Z"});
            if let Some(price) = &price { bar["o"] = price.clone(); }
            let body = if crypto { json!({"bars":{"BTC/USD":[bar]}}) } else { json!({"bars":[bar]}) };
            let mut cache = Cache::default();
            let reader = Reader::new(&cache, 120);
            let _ = reader.candles(&venue(), &ticker(crypto), At(0), 60, true);
            cache.fill(&Reply(Ok(HttpResponse { status: 200, body: body.to_string() })), reader.demands(), 120).await;
            let reader = Reader::new(&cache, 120);
            let result = reader.candles(&venue(), &ticker(crypto), At(0), 60, true);
            if price == Some(json!(0)) || price == Some(json!("12.5")) {
                let bars = result.unwrap();
                assert_eq!(bars.len(), 1);
                assert_eq!(bars[0].1.to_s_f(), if price == Some(json!(0)) { "0.0" } else { "12.5" });
                assert!(!reader.failed());
            } else {
                assert!(result.is_err(), "unreadable candle became a price: {price:?}");
                assert!(reader.failed(), "unreadable candle escaped publication guard: {price:?}");
            }
            assert!(reader.demands().is_empty());
        }
    }
}
