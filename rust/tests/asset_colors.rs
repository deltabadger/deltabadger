//! A21 F25: Asset::InferColorFromImageJob against what Rails recorded (script/rust/record_asset_colors.rb →
//! fixtures/asset_colors.json, the logos in fixtures/logos), the quantizer's agreement with ImageMagick
//! (D-COLOR-QUANTIZER), each security property with its hostile input, and the trigger after a CoinGecko refresh.
mod common;
use chrono::{DateTime, Utc};
use common::seed;
use deltabadger::engine::FixedClock;
use deltabadger::jobs::asset_color::{self, Fetcher, InferColor};
use deltabadger::jobs::{coingecko::{Api, Clients, CoinGecko, Throttle}, import, reference, Cx, Db, Job, Outcome, Wakers};
use deltabadger::venue::http::ScriptedTransport;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wiremock::{matchers, Mock, MockServer, ResponseTemplate};

const HOST: &str = "https://coin-images.coingecko.com/coins/images/1/large/";
const NOW: &str = "2026-11-03T12:00:00Z";
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn recorded() -> Value { serde_json::from_str(include_str!("fixtures/asset_colors.json")).unwrap() }
fn logo(name: &str) -> Vec<u8> { std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logos").join(name)).unwrap() }
fn install(provider: &str) -> (tempfile::TempDir, Connection) {
    let dir = common::baseline_install();
    let o = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
    for (k, v) in [("market_data_provider", provider), ("coingecko_api_key", "CG-test-key")] {
        deltabadger::app_config::set(&o.primary, &seed::cipher(), k, v, at(NOW)).unwrap();
    }
    (dir, o.primary)
}
fn reopen(d: &tempfile::TempDir) -> Connection { Connection::open(d.path().join("production.sqlite3")).unwrap() }
fn cx(db: &Db) -> Cx<'static> { Cx { db: db.clone(), clock: Box::leak(Box::new(FixedClock(at(NOW)))), wakers: Wakers::default() } }
/// A server answering every path with `status` and `body`; the fetcher sends CoinGecko's image host to it.
async fn serving(status: u16, body: Vec<u8>) -> (MockServer, Fetcher) {
    let server = MockServer::start().await;
    Mock::given(matchers::any()).respond_with(ResponseTemplate::new(status).set_body_bytes(body)).mount(&server).await;
    let fetcher = Fetcher::rerouted("https://coin-images.coingecko.com", &server.uri(), Duration::from_secs(5));
    (server, fetcher)
}
async fn hits(server: &MockServer) -> usize { server.received_requests().await.unwrap().len() }
fn colors_of(name: &str) -> Option<Vec<String>> { asset_color::dominant_colors(&logo(name), asset_color::sniff(&logo(name))?) }
fn pick_of(name: &str) -> Option<String> { colors_of(name).and_then(|c| asset_color::most_vivid_color(&c)) }

#[test]
fn pins_and_overrides_are_rails() {
    use sha2::{Digest, Sha256};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, hash) in recorded()["sources"].as_object().unwrap() {
        assert_eq!(hex::encode(Sha256::digest(std::fs::read(root.join(path)).unwrap())), hash.as_str().unwrap(), "re-record {path}: script/rust/repin");
    }
    let ours: serde_json::Map<String, Value> = asset_color::OVERRIDES.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    assert_eq!(Value::Object(ours), recorded()["overrides"]);
}

#[test]
fn most_vivid_color_is_rails() {
    for case in recorded()["vivid"].as_array().unwrap() {
        let colors: Vec<String> = serde_json::from_value(case["colors"].clone()).unwrap();
        assert_eq!(json!(asset_color::most_vivid_color(&colors)), case["pick"], "{colors:?}");
    }
}

#[test]
fn sniffing_is_rails() {
    for l in recorded()["logos"].as_array().unwrap() {
        let name = l["file"].as_str().unwrap();
        let ours = asset_color::sniff(&logo(name)).map(|f| match f { image::ImageFormat::Png => "png", image::ImageFormat::Jpeg => "jpg", image::ImageFormat::Gif => "gif", image::ImageFormat::WebP => "webp", _ => "other" });
        assert_eq!(json!(ours), l["coder"], "{name}");
    }
}

/// allowed_source? for every recorded URL under each market-data configuration, the configured host read whatever the
/// provider (here CoinGecko, with a retained market_data_url).
#[test]
fn allow_list_matches_rails() {
    let mut cases = 0;
    for case in recorded()["allowed"].as_array().unwrap() {
        let (_d, c) = install("coingecko");
        if let Some(url) = case["market_data_url"].as_str() { deltabadger::app_config::set(&c, &seed::cipher(), "market_data_url", url, at(NOW)).unwrap(); }
        let host = asset_color::market_host(&c, &seed::cipher(), None).unwrap();
        assert_eq!(json!(asset_color::allowed_source(case["url"].as_str().unwrap(), host.as_deref())), case["allowed"], "{case}");
        cases += 1;
    }
    assert_eq!(cases, 45);
    // No row: ENV['MARKET_DATA_URL'] is read; a row, even an empty one, wins over it.
    let (_d, c) = install("coingecko");
    assert_eq!(asset_color::market_host(&c, &seed::cipher(), Some("http://data-api:3000")).unwrap().as_deref(), Some("data.deltabadger.com"));
    deltabadger::app_config::set(&c, &seed::cipher(), "market_data_url", "", at(NOW)).unwrap();
    assert_eq!(asset_color::market_host(&c, &seed::cipher(), Some("http://data-api:3000")).unwrap(), None);
}

/// Rust's pick for every committed logo, pinned (D-COLOR-QUANTIZER). The measure against ImageMagick's picks:
/// exact = the same hex; near = both vivid within 15° of hue and 0.1 of lightness, or both grey within 0.1 of lightness.
const RUST_PICKS: &[(&str, &str)] = &[
    ("aave.png", "#9391F7"),
    ("avalanche-2.png", "#E84142"),
    ("binancecoin.png", "#F0B90B"),
    ("bitcoin-small.gif", "#F79217"),
    ("bitcoin.png", "#F7931A"),
    ("bittensor.jpeg", "#FFFFFF"),
    ("chainlink.png", "#0545F6"),
    ("dai.png", "#FAB324"),
    ("ethena-usde.png", "#232323"),
    ("ethereum.png", "#464B76"),
    ("hedera-hashgraph.png", "#000000"),
    ("hyperliquid.jpg", "#96FBE4"),
    ("litecoin.png", "#838383"),
    ("monero.png", "#F26822"),
    ("okb.png", "#000000"),
    ("pepe.jpeg", "#8D2E1C"),
    ("polkadot.jpg", "#000000"),
    ("ripple.png", "#FFFFFF"),
    ("solana.png", "#8853F4"),
    ("stellar.jpg", "#FFFFFF"),
    ("sui.png", "#4DA2FF"),
    ("tether.png", "#019393"),
    ("the-open-network.jpeg", "#0297EB"),
    ("tron.png", "#C53027"),
    ("uniswap.png", "#F50DB4"),
    ("usds.webp", "#FF9A56"),
    ("zcash.png", "#F2BD4C"),
];
/// The logos whose pick matches Rails' exactly, and those that are not even near.
const EXACT: &[&str] = &["aave.png", "avalanche-2.png", "binancecoin.png", "bitcoin.png", "bittensor.jpeg", "hedera-hashgraph.png", "litecoin.png", "monero.png", "okb.png", "polkadot.jpg", "stellar.jpg", "sui.png", "tron.png"];
/// pepe: ImageMagick merges the small red cluster (the lips) away; ours keeps it, and it is the most saturated.
const MISSES: &[&str] = &["pepe.jpeg"];

fn hsl(hex: &str) -> (f64, f64, f64) {
    let ch = |i: usize| f64::from(u8::from_str_radix(&hex[i..i + 2], 16).unwrap()) / 255.0;
    let (r, g, b) = (ch(1), ch(3), ch(5));
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let (d, l) = (max - min, (max + min) / 2.0);
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()) };
    let h = if d == 0.0 { 0.0 } else if max == r { 60.0 * (((g - b) / d).rem_euclid(6.0)) } else if max == g { 60.0 * ((b - r) / d + 2.0) } else { 60.0 * ((r - g) / d + 4.0) };
    (h, s, l)
}
fn near(a: &str, b: &str) -> bool {
    let ((ha, sa, la), (hb, sb, lb)) = (hsl(a), hsl(b));
    let grey = |s: f64, l: f64| s < 0.2 || !(0.1..=0.9).contains(&l);
    let dh = (ha - hb).abs().min(360.0 - (ha - hb).abs());
    (la - lb).abs() <= 0.1 && match (grey(sa, la), grey(sb, lb)) { (true, true) => true, (false, false) => dh <= 15.0, _ => false }
}

#[test]
fn quantizer_agreement_with_imagemagick() {
    let synthetic = ["animated.gif", "halves.png", "tie.png", "transparent.png"];
    let (mut exact, mut nears, mut misses, mut picks, mut real) = (vec![], vec![], vec![], vec![], 0);
    for l in recorded()["logos"].as_array().unwrap() {
        let name = l["file"].as_str().unwrap();
        let Some(rails) = l["pick"].as_str() else { assert_eq!(pick_of(name), None, "{name}: Rails gets no colour"); continue };
        let ours = pick_of(name).unwrap_or_else(|| panic!("{name}: no colour"));
        if synthetic.contains(&name) { assert_eq!(ours, rails, "{name}: a synthetic case must match ImageMagick"); continue; }
        real += 1;
        picks.push((name.to_string(), ours.clone()));
        if ours == rails { exact.push(name.to_string()) } else if near(&ours, rails) { nears.push(name.to_string()) } else { misses.push(name.to_string()) }
        eprintln!("{name:24} rails {rails} rust {ours}{}", if ours == rails { "" } else if near(&ours, rails) { "  near" } else { "  MISS" });
    }
    eprintln!("{real} logos: {} exact, {} near, {} miss ({misses:?})", exact.len(), nears.len(), misses.len());
    assert_eq!(picks.iter().map(|(n, p)| (n.as_str(), p.as_str())).collect::<Vec<_>>(), RUST_PICKS, "Rust's picks moved: re-measure and re-pin");
    assert_eq!(exact, EXACT);
    assert_eq!(misses, MISSES);
}

/// Asset#infer_color_from_image's guards, case by case, on the job itself (bitcoin's logo served).
#[tokio::test(flavor = "current_thread")]
async fn guards_are_rails() {
    let bitcoin = pick_of("bitcoin.png").unwrap();
    for case in recorded()["guards"].as_array().unwrap() {
        let (d, c) = install(case["provider"].as_str().unwrap());
        if case["provider"] == "deltabadger" {
            for (k, v) in [("market_data_url", "https://data.example.com"), ("market_data_token", "t")] { deltabadger::app_config::set(&c, &seed::cipher(), k, v, at(NOW)).unwrap(); }
        }
        c.execute("INSERT INTO assets (external_id, symbol, name, category, color, image_url, created_at, updated_at) VALUES (?1, 'X', 'X', 'Cryptocurrency', ?2, ?3, '2026-01-01', '2026-01-01')",
                  rusqlite::params![case["external_id"].as_str(), case["color_before"].as_str(), case["image_url"].as_str()]).unwrap();
        let id = c.last_insert_rowid();
        let (server, fetcher) = serving(200, logo("bitcoin.png")).await;
        let job = InferColor { asset: id, fetcher, env_market_url: None };
        job.run(cx(&Db::new(c, seed::cipher())), vec![]).await;
        let after: Option<String> = reopen(&d).query_row("SELECT color FROM assets WHERE id = ?1", [id], |r| r.get(0)).unwrap();
        // Rails' bitcoin colour is ImageMagick's; ours is our quantizer's (the same here, pinned above).
        let expected = if case["color_after"] == "#F7931A" && case["requests"] == 1 { json!(bitcoin) } else { case["color_after"].clone() };
        assert_eq!((json!(after), json!(hits(&server).await)), (expected, case["requests"].clone()), "{}", case["case"]);
    }
}

/// A 3xx is an answer (its body sniffed), never followed; 4xx/5xx and an empty body give nothing.
#[tokio::test(flavor = "current_thread")]
async fn statuses_are_rails_and_a_redirect_is_not_followed() {
    let elsewhere = MockServer::start().await;
    Mock::given(matchers::any()).respond_with(ResponseTemplate::new(200).set_body_bytes(logo("bitcoin.png"))).expect(0).mount(&elsewhere).await;
    for case in recorded()["statuses"].as_array().unwrap() {
        let status = case["status"].as_u64().unwrap() as u16;
        let server = MockServer::start().await;
        let body = if status == 204 { vec![] } else { logo("bitcoin.png") };
        Mock::given(matchers::any()).respond_with(ResponseTemplate::new(status).insert_header("Location", format!("{}/x.png", elsewhere.uri()).as_str()).set_body_bytes(body)).mount(&server).await;
        let fetcher = Fetcher::rerouted("https://coin-images.coingecko.com", &server.uri(), Duration::from_secs(5));
        let ours = asset_color::dominant_colors_from_url(&fetcher, &format!("{HOST}status.png"), None).await;
        assert_eq!(ours.is_some(), !case["colors"].is_null(), "HTTP {status}");
        assert_eq!(hits(&server).await, 1, "HTTP {status}: the allowed request arrived");
    }
    assert_eq!(hits(&elsewhere).await, 0, "no redirect followed");
}

#[tokio::test(flavor = "current_thread")]
async fn disallowed_host_makes_no_request() {
    let (server, fetcher) = serving(200, logo("bitcoin.png")).await;
    for url in ["http://coin-images.coingecko.com/x.png", "https://coin-images.coingecko.com.evil.com/x.png", "https://coin-images.coingecko.com@evil.com/x.png",
                "https://127.0.0.1/x.png", "https://evil.com/x.png", "https://coin-images.coingecko.com/a b.png"] {
        assert_eq!(asset_color::dominant_colors_from_url(&fetcher, url, None).await, None, "{url}");
    }
    assert_eq!(hits(&server).await, 0);
    assert!(asset_color::dominant_colors_from_url(&fetcher, &format!("{HOST}ok.png"), None).await.is_some());
    assert_eq!(hits(&server).await, 1, "the allowed URL did arrive");
}

/// Utilities::Image::MAX_BYTES: more than 5,000,000 bytes is nothing; exactly that many is read.
#[tokio::test(flavor = "current_thread")]
async fn oversize_body_is_dropped() {
    let mut big = logo("bitcoin.png");
    big.resize(asset_color::MAX_BYTES + 1, 0);
    let (server, fetcher) = serving(200, big.clone()).await;
    assert_eq!(fetcher.fetch(&format!("{HOST}big.png")).await, None);
    assert_eq!(hits(&server).await, 1);
    big.truncate(asset_color::MAX_BYTES);
    let (_server, fetcher) = serving(200, big).await;
    assert_eq!(fetcher.fetch(&format!("{HOST}big.png")).await.map(|b| b.len()), Some(asset_color::MAX_BYTES));
}

#[tokio::test(flavor = "current_thread")]
async fn slow_server_times_out() {
    let server = MockServer::start().await;
    Mock::given(matchers::any()).respond_with(ResponseTemplate::new(200).set_body_bytes(logo("bitcoin.png")).set_delay(Duration::from_secs(2))).mount(&server).await;
    let fetcher = Fetcher::rerouted("https://coin-images.coingecko.com", &server.uri(), Duration::from_millis(200));
    let started = Instant::now();
    assert_eq!(asset_color::dominant_colors_from_url(&fetcher, &format!("{HOST}slow.png"), None).await, None);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(hits(&server).await, 1);
    assert_eq!(asset_color::TIMEOUT, Duration::from_secs(5), "Utilities::Image::TIMEOUT");
}

/// An SVG with an external entity and an MSL script, served as image/png from a .png URL: refused by the sniff, no colour.
#[tokio::test(flavor = "current_thread")]
async fn svg_and_msl_get_no_colour() {
    for name in ["evil.svg", "evil.msl"] {
        assert_eq!(asset_color::sniff(&logo(name)), None);
        let server = MockServer::start().await;
        Mock::given(matchers::any()).respond_with(ResponseTemplate::new(200).insert_header("Content-Type", "image/png").set_body_bytes(logo(name))).mount(&server).await;
        let fetcher = Fetcher::rerouted("https://coin-images.coingecko.com", &server.uri(), Duration::from_secs(5));
        assert_eq!(asset_color::dominant_colors_from_url(&fetcher, &format!("{HOST}logo.png"), None).await, None, "{name}");
        assert_eq!(hits(&server).await, 1);
    }
}

/// A 48 KB PNG declaring 20000×20000 pixels: refused by the limits before any pixel buffer, quickly (D-COLOR-IMAGE-LIMITS).
#[test]
fn bomb_is_refused() {
    let started = Instant::now();
    assert_eq!(colors_of("bomb.png"), None);
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
}

/// A 1×1 PNG whose zTXt chunk inflates to 64 MB: finishes quickly, whatever it decides.
#[test]
fn metadata_bomb_is_bounded() {
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(1, 1, image::Rgba([247, 147, 26, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    std::io::Write::write_all(&mut z, &vec![b'a'; 64 << 20]).unwrap();
    let data = [b"Comment\0\0".to_vec(), z.finish().unwrap()].concat();
    let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
    let body = [b"zTXt".to_vec(), data].concat();
    chunk.extend_from_slice(&body);
    let crc = crc32(&body);
    chunk.extend_from_slice(&crc.to_be_bytes());
    let ihdr_end = 8 + 25; // signature, then IHDR's length, type, 13 bytes and CRC
    png.splice(ihdr_end..ihdr_end, chunk);
    let started = Instant::now();
    let _ = asset_color::dominant_colors(&png, image::ImageFormat::Png);
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 { crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 }; }
    }
    !crc
}

/// The largest image the limits allow still gets a colour, in bounded time; one pixel wider gets none.
#[test]
fn largest_allowed_decodes() {
    let side = asset_color::MAX_SIDE;
    let encode = |w: u32, h: u32| {
        let img = image::RgbaImage::from_fn(w, h, |x, _| if x < w / 2 { image::Rgba([247, 147, 26, 255]) } else { image::Rgba([255, 255, 255, 255]) });
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    };
    let started = Instant::now();
    let colors = asset_color::dominant_colors(&encode(side, side), image::ImageFormat::Png).unwrap();
    assert_eq!(asset_color::most_vivid_color(&colors).as_deref(), Some("#F7931A"));
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    assert_eq!(asset_color::dominant_colors(&encode(side + 1, 1), image::ImageFormat::Png), None);
}

/// Every GIF frame counts (recorded: animated.gif), up to the frame cap.
#[test]
fn gif_frames_count_up_to_the_budget() {
    assert_eq!(colors_of("animated.gif").unwrap().len(), 3);
    let frames: Vec<image::Frame> = (0..80).map(|i| image::Frame::new(image::RgbaImage::from_pixel(4, 4, image::Rgba([i as u8, 0, 0, 255])))).collect();
    let mut gif = Vec::new();
    image::codecs::gif::GifEncoder::new(&mut gif).encode_frames(frames).unwrap();
    // 64 frames of 16 pixels each: every frame's one colour is 1/64 of the pixels, over the 1 % threshold; five are kept.
    assert_eq!(asset_color::dominant_colors(&gif, image::ImageFormat::Gif).unwrap().len(), 5);
}

/// One decode at a time, and a caller dropped while waiting never starts one.
#[tokio::test(flavor = "current_thread")]
async fn decodes_never_overlap() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (entered, release) = (Arc::new(AtomicUsize::new(0)), Arc::new(std::sync::Barrier::new(2)));
    let (e, r) = (entered.clone(), release.clone());
    let first = tokio::spawn(asset_color::decode_exclusive(move || { e.fetch_add(1, Ordering::SeqCst); r.wait(); }));
    while entered.load(Ordering::SeqCst) == 0 { tokio::task::yield_now().await; }
    for _ in 0..3 {
        let e = entered.clone();
        let waiting = asset_color::decode_exclusive(move || { e.fetch_add(1, Ordering::SeqCst); });
        assert!(tokio::time::timeout(Duration::from_millis(50), waiting).await.is_err(), "a waiter ran while the first decoded");
    }
    assert_eq!(entered.load(Ordering::SeqCst), 1);
    tokio::task::spawn_blocking(move || release.wait()).await.unwrap();
    first.await.unwrap().unwrap();
    let e = entered.clone();
    asset_color::decode_exclusive(move || { e.fetch_add(1, Ordering::SeqCst); }).await.unwrap();
    assert_eq!(entered.load(Ordering::SeqCst), 2, "the dropped waiters never decoded; the next caller did");
}

fn clients(t: &ScriptedTransport) -> Clients {
    let t = t.clone();
    Rc::new(move |key: &str| Box::new(CoinGecko::new(key, t.clone()).with_throttle(Arc::new(Throttle::new(10_000)))) as Box<dyn Api>)
}

/// market_data.rb:86: the CoinGecko refresh wakes the colour job for exactly the assets whose image_url changed.
#[tokio::test(flavor = "current_thread")]
async fn a_changed_image_wakes_its_colour_job() {
    let (_d, c) = install("coingecko");
    let mut ids = vec![];
    for (ext, image) in [("fresh", None), ("same", Some("https://img/same.png")), ("moved", Some("https://img/old.png")), ("absent", Some("https://img/absent.png"))] {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, image_url, created_at, updated_at) VALUES (?1, ?1, ?1, 'Cryptocurrency', ?2, '2026-01-01', '2026-01-01')", rusqlite::params![ext, image]).unwrap();
        ids.push(c.last_insert_rowid());
    }
    let coin = |id: &str| json!({ "id": id, "symbol": id, "name": id, "image": format!("https://img/{id}.png"), "market_cap": 1e9, "market_cap_rank": 3, "circulating_supply": 10.5 });
    let t = ScriptedTransport::default();
    t.reply("GET /api/v3/ping", 401, json!({})).reply("GET /api/v3/coins/markets", 200, json!([coin("fresh"), coin("same"), coin("moved")]));
    let db = Db::new(c, seed::cipher());
    let cx = cx(&db);
    let wakers = cx.wakers.clone();
    assert_eq!(reference::run_once_with::<ScriptedTransport>(reference::ASSETS, None, &clients(&t), cx).await, Outcome::Done);
    let woken: Vec<_> = wakers.pending().into_iter().filter(|(n, _)| n == asset_color::NAME).map(|(_, s)| s.unwrap()).collect();
    assert_eq!(woken, [ids[0].to_string(), ids[2].to_string()], "fresh (none → one) and moved; not same, not absent");
}

/// The wakes follow each committed unit: a second unit's failure keeps the first unit's.
#[tokio::test(flavor = "current_thread")]
async fn colour_wakes_follow_each_committed_chunk() {
    let (_d, c) = install("coingecko");
    let mut rows = vec![];
    c.execute_batch("BEGIN").unwrap();
    for i in 0..(deltabadger::jobs::CHUNK + 1) {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?1, ?1, 'Cryptocurrency', '2026-01-01', '2026-01-01')", [format!("c{i}")]).unwrap();
        let cols = import::coingecko_columns(&format!("c{i}"), &json!({ "symbol": format!("c{i}"), "name": "n", "image": format!("https://img/{i}.png") })).unwrap().unwrap();
        rows.push((c.last_insert_rowid(), cols));
    }
    c.execute_batch("COMMIT").unwrap();
    let last = rows.last().unwrap().0;
    c.execute_batch(&format!("CREATE TRIGGER boom BEFORE UPDATE ON assets WHEN NEW.id = {last} BEGIN SELECT RAISE(ABORT, 'boom'); END")).unwrap();
    let db = Db::new(c, seed::cipher());
    let woken = std::cell::RefCell::new(vec![]);
    let result = import::refresh_assets(&db, rows, at(NOW), |ids| woken.borrow_mut().extend(ids)).await;
    assert!(result.is_err());
    assert_eq!(woken.borrow().len(), deltabadger::jobs::CHUNK, "the first unit's wakes, none from the failed one");
    assert!(!woken.borrow().contains(&last));
}
