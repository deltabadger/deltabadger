mod common;
use common::seed::{self, BotSpec};
use deltabadger::engine::{run::{self, Engine}, FixedClock};
use deltabadger::jobs::{data_api::{self, Config, DataApi}, reference, state, Scheduler};
use deltabadger::venue::fake::{FakeFactory, FakeVenue};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// A real scheduler records the failed reference run while the engine remains usable in the same runtime.
async fn recorded_failure(config: Config) -> String {
    let (dir, o, s) = common::install();
    let now = "2026-09-01T10:00:01Z".parse().unwrap();
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let paths = deltabadger::store::Paths::from_env(&|_| None, dir.path());
    let lock = deltabadger::lease::lock(&paths, now).unwrap();
    let venue = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000", "49995")
        .balance_body("ZEUR", "10000", "0");
    let mut engine = Engine::new(o.primary, FakeFactory(venue), seed::cipher(), lock);
    let jobs = reference::jobs(Some(DataApi::live(config))).into_iter().filter(|j| j.spec().name == reference::ASSETS).collect();
    let scheduler = Scheduler::new(rusqlite::Connection::open(&paths.primary).unwrap(), seed::cipher(), jobs, None);
    let (stop, rx) = tokio::sync::watch::channel(false);
    let clock = FixedClock(now);
    let check = async {
        loop {
            let state = state::read(&engine.primary, reference::ASSETS, None).unwrap();
            if let Some(error) = state.last_error {
                assert!(state.last_success_at.is_none());
                run::step(&mut engine, &clock).await.unwrap();
                let orders: i64 = engine.primary.query_row("SELECT count(*) FROM transactions", [], |r| r.get(0)).unwrap();
                assert_eq!(orders, 1, "the engine still places its unrelated order after the job fails");
                stop.send(true).unwrap();
                break error;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    let (result, error) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        tokio::join!(scheduler.run(rx, &clock), check)
    }).await.expect("the failed job must not stall the scheduler or engine");
    result.unwrap();
    error
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_market_data_urls_are_recorded_failures_and_the_engine_keeps_running() {
    for from_setting in [false, true] {
        for url in ["", "http://[malformed", "http://", "file:///tmp/feed"] {
            let (_dir, o, _) = common::install();
            let now = "2026-09-01T10:00:01Z".parse().unwrap();
            deltabadger::app_config::set(&o.primary, &seed::cipher(), "market_data_provider", "deltabadger", now).unwrap();
            if from_setting { deltabadger::app_config::set(&o.primary, &seed::cipher(), "market_data_url", url, now).unwrap(); }
            let config = data_api::config(&|key| (key == "MARKET_DATA_URL").then(|| url.to_string()), &o.primary, &seed::cipher()).unwrap().unwrap();
            assert!(recorded_failure(config).await.contains("URL"));
        }
    }
}

async fn oversized_response(endless: bool) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(socket);
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).await.unwrap() > 0);
            if line == "\r\n" { break; }
        }
        let mut socket = reader.into_inner();
        // No Content-Length: the reader must enforce the bound while chunks arrive.
        socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
        let chunk = vec![b' '; 64 * 1024];
        for i in 0.. {
            if !endless && i == 600 {
                let _ = socket.write_all(b"0\r\n\r\n").await;
                break;
            }
            if socket.write_all(b"10000\r\n").await.is_err() || socket.write_all(&chunk).await.is_err() || socket.write_all(b"\r\n").await.is_err() { break; }
            tokio::task::yield_now().await;
        }
    });
    let error = recorded_failure(Config { url, token: String::new() }).await;
    assert!(error.contains("response body is over"), "{error}");
    tokio::time::timeout(std::time::Duration::from_secs(2), server).await.unwrap().unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn oversized_reference_body_is_a_recorded_job_failure() { oversized_response(false).await; }

#[tokio::test(flavor = "current_thread")]
async fn never_ending_reference_body_is_cut_off_and_recorded_as_a_failure() { oversized_response(true).await; }
