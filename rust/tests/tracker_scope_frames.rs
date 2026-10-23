//! The scope frames and the tracker's Holdings tab (3.0, Rust only; plan 2026-10-23 tracker tabs and scope frames):
//! what a frame load answers, what a full page carries, and the record's three panes. The grids compare the rest with
//! Rails through the listed transform (support/tabs_3_0.rs).
mod common;
#[path = "support/tracker_fx.rs"]
mod fx;
use deltabadger::jobs::Outcome;
use fx::*;

/// Alpaca (dollars and bitcoin) and KuCoin (ether and tether), the ledger cached: a populated page with two venues.
async fn two_venues() -> (Fx, i64) {
    let fx = alpaca_synced("0.01").await;
    let (kucoin, key) = add_kucoin(&fx);
    kucoin_balances(&fx, kucoin, key, &[("ETH", "0.004", "2000"), ("USDT", "500", "1")]);
    assert_eq!(run(&fx).await, Outcome::Done);
    (fx, kucoin)
}

/// A working bot and a stopped one, so the bot list offers its status filter.
fn two_bots(fx: &Fx) {
    use common::seed::{insert_bot, BotSpec};
    let c = fx.c();
    insert_bot(&c, &fx.s, &BotSpec::weekly(10.0, "2026-09-01 00:00:00"));
    insert_bot(&c, &fx.s, &BotSpec { status: 2, ..BotSpec::weekly(20.0, "2026-09-01 00:00:00") });
}

async fn framed(fx: &Fx, app: &deltabadger::web::App, path: &str, frame: &str) -> String {
    let answer = browser(fx, app).send(app, "GET", path, None, common::web::Csrf::None, &[("turbo-frame", frame)]).await;
    assert_eq!(answer.status, 200, "{path}: {}", &answer.body[..answer.body.len().min(400)]);
    answer.body
}

/// The text between `from` and the next `to` after it.
fn between<'a>(body: &'a str, from: &str, to: &str) -> &'a str {
    let start = body.find(from).unwrap_or_else(|| panic!("{from} missing"));
    &body[start..start + body[start..].find(to).unwrap_or_else(|| panic!("{to} missing after {from}"))]
}

/// A load of the tracker's frame sends the bar's far end along, stating the scope it loaded; a whole page states it in
/// place and sends nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_frame_load_sends_the_bars_far_end_for_its_scope() {
    let (fx, kucoin) = two_venues().await;
    let app = app(&fx);
    let path = format!("/tracker?exchange_id={kucoin}");
    let frame = framed(&fx, &app, &path, "tracker_scope").await;
    assert!(frame.contains("<turbo-frame id=\"tracker_scope\" class=\"scope-frame\" target=\"_top\" data-turbo-action=\"advance\">"), "{frame}");
    let stream = between(&frame, "<turbo-stream action=\"replace\" target=\"tracker_bar_end\">", "</turbo-stream>");
    assert!(stream.contains(&format!("/tracker/export?exchange_id={kucoin}")), "the export link carries the scope: {stream}");
    assert!(frame.find("<turbo-stream").unwrap() > frame.find("<turbo-frame id=\"tracker_scope\"").unwrap() && frame.find("<turbo-stream").unwrap() < frame.find("</turbo-frame>").unwrap(), "the stream is inside the frame");

    let page = get(&fx, &app, &path).await.body;
    assert!(!page.contains("<turbo-stream"), "a whole page sends no stream");
    assert!(page.contains("<div class=\"tracker-exchanges__end buttons-no-outline\" id=\"tracker_bar_end\">"));
    // The switch stays outside the frame and names it; a venue's link loads into it.
    let bar = between(&page, "<div class=\"tracker-exchanges\">", "<turbo-frame id=\"tracker_scope\"");
    assert!(bar.contains("data-segmented-frame=\"tracker_scope\""), "{bar}");
    assert!(bar.contains(&format!("data-value=\"{kucoin}\" data-turbo-frame=\"tracker_scope\" data-segmented-target=\"option\"")), "{bar}");
}

/// The record opens on Holdings: three panes, Holdings on; the card unfolded, each row carrying its class; the class
/// filter offers the classes the rows hold.
#[tokio::test(flavor = "current_thread")]
async fn the_record_opens_on_holdings_with_a_class_filter() {
    let (fx, _) = two_venues().await;
    // Cash is shown, so tether has its row beside the crypto ones.
    fx.c().execute("UPDATE users SET tracker_settings='{\"show_cash\":true}'", []).unwrap();
    let app = app(&fx);
    let page = get(&fx, &app, "/tracker").await.body;
    let switch = between(&page, "<div class=\"tracker-record__switch filters\"", "</div>\n\n");
    let values: Vec<&str> = switch.match_indices("data-value=\"").map(|(at, _)| { let v = &switch[at + 12..]; &v[..v.find('"').unwrap()] }).collect();
    assert_eq!(values, ["hold", "pos", "tx"], "{switch}");
    assert!(switch.contains("class=\"segmented__option is-on\" role=\"radio\" aria-checked=\"true\" tabindex=\"0\" data-value=\"hold\""), "{switch}");
    assert!(page.contains("<div class=\"tracker-record__pane\" data-pane=\"hold\""));
    for pane in ["pos", "tx"] { assert!(page.contains(&format!("<div class=\"tracker-record__pane tracker-record__pane--off\" data-pane=\"{pane}\"")), "{pane}"); }
    assert!(!page.contains("tracker-holdings__more"), "no Show more");

    let pane = between(&page, "data-pane=\"hold\"", "data-pane=\"tx\"");
    let mut classes: Vec<&str> = vec![];
    for (at, _) in pane.match_indices("<div class=\"tracker-holdings__row\" data-order-filter-target=\"row\" data-order-type=\"all") {
        let rest = &pane[at..];
        let kind = &rest[rest.find("data-order-type=\"all").unwrap() + 20..rest.find("\">").unwrap()];
        if let Some(class) = kind.strip_prefix(' ') { if !classes.contains(&class) { classes.push(class); } }
    }
    assert!(classes.contains(&"crypto"), "bitcoin and ether are crypto: {pane}");
    assert!(classes.contains(&"cash"), "tether is cash: {pane}");
    let filter = between(pane, "<div class=\"tracker-record__filters filters\"", "<div class=\"tracker-record__table\">");
    for class in ["crypto", "stocks", "cash"] {
        assert_eq!(filter.contains(&format!("data-value=\"{class}\"")), classes.contains(&class), "{class}: {filter}");
    }
    assert!(filter.contains("data-value=\"all\""), "{filter}");
}

/// A load of the bot list's frame sends the head's actions along; a whole page has them in place.
#[tokio::test(flavor = "current_thread")]
async fn a_bots_frame_load_sends_the_heads_actions() {
    let (fx, _) = two_venues().await;
    two_bots(&fx);
    let app = app(&fx);
    let frame = framed(&fx, &app, "/bots?filter=all", "bots_scope").await;
    assert!(frame.contains("<turbo-frame id=\"bots_scope\" class=\"scope-frame\" target=\"_top\" data-turbo-action=\"advance\">"), "{frame}");
    let stream = between(&frame, "<turbo-stream action=\"replace\" target=\"bots_actions\">", "</turbo-stream>");
    assert!(stream.contains("<div class=\"page-head__actions\" id=\"bots_actions\">"), "{stream}");
    assert!(frame.find("<turbo-stream").unwrap() < frame.find("</turbo-frame>").unwrap(), "the stream is inside the frame");
    let page = get(&fx, &app, "/bots").await.body;
    assert!(!page.contains("<turbo-stream"), "a whole page sends no stream");
    assert!(page.contains("<div class=\"merge-bar\" data-bots-merge-target=\"bar\" hidden>"), "the merge bar is always there");
    let head = between(&page, "<div class=\"page-head page-head--bots\">", "<turbo-frame id=\"bots_scope\"");
    assert!(head.contains("data-segmented-frame=\"bots_scope\""), "{head}");
    assert!(head.contains("data-action=\"segmented#select\" data-turbo-frame=\"bots_scope\" href="), "{head}");
}

/// In headless Chrome over the served app (a synthetic install): the venue switch and the bot list's filter load into
/// their frames and the switch is never rebuilt; navigations run one at a time; Back and Forward restore what each
/// step showed; a failed or frameless answer leaves the page sound; Merge waits for a filter in flight; the record opens
/// on Holdings and its class filter hides the other classes. Needs Chrome and bun:
/// `cargo-locked.sh nextest run --test tracker_scope_frames --run-ignored only`.
#[tokio::test(flavor = "current_thread")]
#[ignore = "needs Chrome and bun"]
async fn a_real_browser_switches_scopes_in_place() {
    let (fx, kucoin) = two_venues().await;
    let (broken, broken_key) = add_venue(&fx, "Exchanges::Binance", "Binance", 0);
    fx.c().execute("UPDATE api_keys SET status=2 WHERE id=?1", [broken_key]).unwrap();
    two_bots(&fx);
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    fx.c().execute("UPDATE users SET encrypted_password=?1, wash_sale_enabled=0, tracker_settings='{\"show_cash\":true}'", [hash]).unwrap();
    let app = common::web::app(fx._dir.path(), common::web::SECRET, common::web::TestClock::starting(chrono::Utc::now()));
    for (k, value) in [("market_data_provider", "deltabadger"), ("market_data_url", "http://127.0.0.1:1"), ("market_data_token", "synthetic-token")] {
        deltabadger::app_config::set(&fx.c(), &app.cipher, k, value, app.now()).unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { let _ = deltabadger::web::server::serve_on(listener, app, Default::default()).await; });
    let script = tempfile::tempdir().unwrap();
    std::fs::write(script.path().join("check.mjs"), BROWSER_CHECK).unwrap();
    let mut check = std::process::Command::new("bun");
    check.arg(script.path().join("check.mjs")).env("BASE_URL", format!("http://{address}")).env("EMAIL", "o@example.com").env("PASSWORD", "Correct-horse-9")
        .env("SHOTS", std::env::var("SCOPE_SHOTS").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("scope-shots")))
        .env("ALPACA", fx.s.exchange_id.to_string()).env("KUCOIN", kucoin.to_string()).env("BROKEN", broken.to_string());
    let output = tokio::task::spawn_blocking(move || check.output()).await.unwrap().unwrap();
    server.abort();
    let log = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    eprintln!("{log}");
    assert!(output.status.success(), "{log}");
}
const BROWSER_CHECK: &str = r#####"
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
const { BASE_URL, EMAIL, PASSWORD, ALPACA, KUCOIN, BROKEN, SHOTS } = process.env;
mkdirSync(SHOTS, { recursive: true });
const chrome = [process.env.CHROME, "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/usr/bin/google-chrome", "/usr/bin/chromium"].filter(Boolean).find(existsSync);
const profile = mkdtempSync(join(tmpdir(), "scope-check-"));
const browser = spawn(chrome, ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "--window-size=1280,900", "about:blank"], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(what, check, ms = 15000) { const end = Date.now() + ms; let last; for (;;) { last = await check(); if (last && last.ok !== false) return last; if (Date.now() > end) throw new Error(`timed out waiting for ${what}: ${JSON.stringify(last)}`); await sleep(50); } }
const errors = [];
const failures = [];
const expect = (what, ok, detail) => { if (!ok) failures.push(`${what}: ${JSON.stringify(detail)}`); else console.log(`ok ${what}`); };
// What the Fetch domain does with an intercepted scope request: let it through, hold it, fail it, or answer 500.
let mode = "continue";
const held = [];
const requested = [];
try {
  const portFile = join(profile, "DevToolsActivePort");
  const port = await until("Chrome", () => existsSync(portFile) && readFileSync(portFile, "utf8").split("\n")[0]);
  const target = await until("a page", async () => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === "page"));
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let id = 0; const waiting = new Map();
  const send = (method, params = {}) => new Promise((resolve) => { const n = ++id; waiting.set(n, resolve); socket.send(JSON.stringify({ id: n, method, params })); });
  const boom = Buffer.from("<!DOCTYPE html><html><body><p id=\"boom\">boom</p></body></html>").toString("base64");
  socket.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (m.method === "Log.entryAdded" && m.params.entry.level === "error" && !m.params.entry.text.includes("500")) errors.push(m.params.entry.text);
    if (m.method === "Runtime.exceptionThrown") errors.push(m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text);
    if (m.method === "Fetch.requestPaused") {
      const { requestId, request } = m.params;
      // Only a scope's page is acted on; everything else passes.
      if (!/[?&](exchange_id|filter)=/.test(request.url)) { send("Fetch.continueRequest", { requestId }); return; }
      requested.push(request.url);
      if (mode === "hold") held.push(requestId);
      else if (mode === "fail") send("Fetch.failRequest", { requestId, errorReason: "Failed" });
      else if (mode === "500") send("Fetch.fulfillRequest", { requestId, responseCode: 500, responseHeaders: [{ name: "Content-Type", value: "text/html; charset=utf-8" }], body: boom });
      else send("Fetch.continueRequest", { requestId });
    }
    if (waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
  };
  const evaluate = async (expression) => (await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true })).result?.result?.value;
  const release = async () => { mode = "continue"; while (held.length) await send("Fetch.continueRequest", { requestId: held.shift() }); };
  await send("Log.enable"); await send("Runtime.enable");
  await send("Page.navigate", { url: `${BASE_URL}/login` });
  await until("the login form", () => evaluate(`!!document.querySelector("#user_password")`));
  await evaluate(`(() => { document.querySelector("#user_email").value = ${JSON.stringify(EMAIL)}; document.querySelector("#user_password").value = ${JSON.stringify(PASSWORD)}; document.querySelector("#user_password").form.requestSubmit(); })()`);
  await until("signed in", () => evaluate(`location.pathname !== "/login" && !!document.querySelector("main.main")`));

  // ---- the tracker ----
  const SW = `document.querySelector('[data-segmented-frame="tracker_scope"]')`;
  const state = (frame) => evaluate(`(() => { const s = document.querySelector('[data-segmented-frame="${frame}"]'); if (!s) return null; const v = (q) => s.querySelector(q)?.dataset.value ?? null; const f = document.getElementById("${frame}"); const end = document.querySelector('#tracker_bar_end a.dropdown__item[href*="/tracker/export"]'); return { url: location.pathname + location.search, current: v('[aria-current="page"]'), on: v('.segmented__option.is-on'), pending: !!f?.hasAttribute("data-segmented-pending"), same: s === window.__switch, export: end?.getAttribute("href") ?? null, length: history.length }; })()`);
  const click = (frame, value) => evaluate(`document.querySelector('[data-segmented-frame="${frame}"] [data-value="${value}"]').click()`);
  const settled = (frame, value, url) => until(`${frame} on ${value}`, async () => { const s = await state(frame); return s && !s.pending && s.current === value && s.on === value && s.url === url ? s : { ok: false, ...s }; });
  const open = async (path, frame) => {
    await send("Page.navigate", { url: `${BASE_URL}${path}` });
    await until(`${path} connected`, () => evaluate(`(() => { const s = document.querySelector('[data-segmented-frame="${frame}"]'); return !!s && !!s.querySelector(".segmented__thumb").style.width; })()`));
    await evaluate(`window.__switch = document.querySelector('[data-segmented-frame="${frame}"]'); true`);
  };
  await open("/tracker", "tracker_scope");
  const shot = async (name) => writeFileSync(join(SHOTS, `${name}.png`), Buffer.from((await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: true })).result.data, "base64"));
  await sleep(500);
  await shot("tracker-1280");
  await send("Emulation.setDeviceMetricsOverride", { width: 375, height: 800, deviceScaleFactor: 2, mobile: true });
  await sleep(500);
  await shot("tracker-375");
  expect("no sideways scroll at 375", !(await evaluate(`document.documentElement.scrollWidth > document.documentElement.clientWidth`)), true);
  await send("Emulation.clearDeviceMetricsOverride");
  await sleep(300);

  // Holdings is the pane on, and its Crypto tab hides every row of another class.
  const holdings = await evaluate(`(() => { const pane = document.querySelector('[data-pane="hold"]'); return { on: !pane.classList.contains("tracker-record__pane--off"), offs: [...document.querySelectorAll('[data-pane="pos"], [data-pane="tx"]')].every((p) => p.classList.contains("tracker-record__pane--off")) }; })()`);
  expect("Holdings is on by default", holdings.on && holdings.offs, holdings);
  await evaluate(`document.querySelector('[data-pane="hold"] .segmented__option[data-value="crypto"]').click()`);
  const filtered = await until("the Crypto tab", () => evaluate(`(() => { const rows = [...document.querySelectorAll('[data-pane="hold"] .tracker-holdings__row')]; const shown = rows.filter((r) => r.offsetParent); return { ok: shown.length > 0 && shown.length < rows.length, all: rows.length, shown: shown.map((r) => r.dataset.orderType) }; })()`));
  expect("the Crypto tab shows crypto rows only", filtered.shown.every((t) => t.split(" ").includes("crypto")), filtered);
  await shot("holdings-crypto");
  await evaluate(`document.querySelector('[data-pane="hold"] .segmented__option[data-value="all"]').click()`);

  // A → B → C, then Back, Back, Forward: URL, selection and the bar's far end agree at every step; the switch is the
  // same element across the frame loads.
  await click("tracker_scope", ALPACA);
  let s = await settled("tracker_scope", ALPACA, `/tracker?exchange_id=${ALPACA}`);
  expect("B: the switch stays", s.same, s); expect("B: the far end states B", s.export?.includes(`exchange_id=${ALPACA}`), s);
  await click("tracker_scope", KUCOIN);
  s = await settled("tracker_scope", KUCOIN, `/tracker?exchange_id=${KUCOIN}`);
  expect("C: the switch stays", s.same, s); expect("C: the far end states C", s.export?.includes(`exchange_id=${KUCOIN}`), s);
  await evaluate(`history.back()`);
  s = await settled("tracker_scope", ALPACA, `/tracker?exchange_id=${ALPACA}`);
  expect("Back: B's far end", s.export?.includes(`exchange_id=${ALPACA}`), s);
  await evaluate(`history.back()`);
  s = await settled("tracker_scope", "all", "/tracker");
  expect("Back: A's far end", s.export === "/tracker/export", s);
  await evaluate(`history.forward()`);
  s = await settled("tracker_scope", ALPACA, `/tracker?exchange_id=${ALPACA}`);
  expect("Forward: B again", s.export?.includes(`exchange_id=${ALPACA}`), s);

  // One at a time: B held, C and D clicked meanwhile — the chip follows at once, C is never asked for, D follows B.
  await open("/tracker", "tracker_scope");
  console.log(`Fetch.enable: ${JSON.stringify(await send("Fetch.enable", { patterns: [{ urlPattern: "*", requestStage: "Request" }] }))}`);
  const start = (await state("tracker_scope")).length;
  mode = "hold"; requested.length = 0;
  await click("tracker_scope", KUCOIN);
  await until("B held", async () => held.length === 1 || { ok: false, requested, held: held.length, state: await state("tracker_scope") });
  await click("tracker_scope", ALPACA);
  await click("tracker_scope", "all");
  s = await state("tracker_scope");
  expect("the chip follows the last click at once", s.on === "all" && s.pending, s);
  await release();
  s = await settled("tracker_scope", "all", "/tracker");
  expect("C was never requested", !requested.some((u) => u.includes(`exchange_id=${ALPACA}`)), requested);
  expect("history holds B and D", s.length === start + 2, { start, s });
  expect("the switch stays through the race", s.same, s);

  // B outlasts the five seconds while C is queued: C opens as a whole page. B's answer arriving before C's page is
  // refused, so it neither renders nor takes the history; C's page then lands as chosen.
  mode = "hold";
  await click("tracker_scope", KUCOIN);
  await until("B held again", () => held.length === 1);
  await click("tracker_scope", ALPACA);
  await until("C's whole-page request held", () => held.length === 2, 12000);
  await send("Fetch.continueRequest", { requestId: held.shift() });
  await sleep(1000);
  s = await state("tracker_scope");
  expect("B's late answer is refused", s && !s.url.includes(`exchange_id=${KUCOIN}`) && s.current !== KUCOIN, s);
  await release();
  await until("C as a whole page", async () => { const s = await state("tracker_scope"); return s && !s.same && s.url === `/tracker?exchange_id=${ALPACA}` && s.current === ALPACA ? s : { ok: false, ...s }; });
  await open("/tracker", "tracker_scope");

  // A network failure: the chip goes back to the scope shown. An error page without the frame: shown whole.
  mode = "fail";
  await click("tracker_scope", KUCOIN);
  s = await settled("tracker_scope", "all", "/tracker");
  expect("a network failure puts the chip back", s.same, s);
  mode = "500";
  await click("tracker_scope", KUCOIN);
  await until("the error page, whole", () => evaluate(`!!document.getElementById("boom")`));
  mode = "continue";

  // A broken venue's chip opens its key form in the modal and selects nothing.
  await open("/tracker", "tracker_scope");
  await click("tracker_scope", BROKEN);
  await until("the key modal", () => evaluate(`!!document.querySelector("turbo-frame#modal dialog[open], turbo-frame#modal .modal")`));
  s = await state("tracker_scope");
  expect("a broken chip moves nothing", s.on === "all" && s.current === "all" && s.url === "/tracker", s);

  // ---- the bot list ----
  await open("/bots", "bots_scope");
  await sleep(500);
  await shot("bots");
  await click("bots_scope", "active");
  s = await settled("bots_scope", "active", "/bots?filter=active");
  expect("bots: the filter stays", s.same, s);
  await click("bots_scope", "all");
  s = await settled("bots_scope", "all", "/bots?filter=all");
  const bar = `document.querySelector('[data-bots-merge-target="bar"]')`;
  const merge = `document.querySelector('#bots_actions [data-action="bots-merge#enter"]')`;
  if (await evaluate(`!!${merge}`)) {
    await evaluate(`${merge}.click()`);
    expect("Merge enters after a filter", await until("the bar", () => evaluate(`!${bar}.hidden`)), true);
    await evaluate(`document.querySelector('[data-action="bots-merge#cancel"]').click()`);
    await until("the bar hidden", () => evaluate(`${bar}.hidden`));
    // A filter in flight: Merge waits for it.
    mode = "hold";
    await click("bots_scope", "active");
    await until("the filter held", () => held.length === 1);
    await evaluate(`${merge}.click()`);
    expect("Merge is refused while a filter loads", await evaluate(`${bar}.hidden`), true);
    await release();
    await settled("bots_scope", "active", "/bots?filter=active");
    // Back to All, then Merge enters.
    await evaluate(`history.back()`);
    await settled("bots_scope", "all", "/bots?filter=all");
    await evaluate(`${merge}.click()`);
    expect("Merge enters after Back", await until("the bar after Back", () => evaluate(`!${bar}.hidden`)), true);
    await evaluate(`document.querySelector('[data-action="bots-merge#cancel"]').click()`);
    await until("the bar hidden again", () => evaluate(`${bar}.hidden`));
  } else {
    failures.push("bots: no Merge offered for two bots on one venue");
  }
  // A tile is a whole visit, out of the frame.
  await evaluate(`document.querySelector('#bots_scope .itiles a[href*="/bots/"]').click()`);
  await until("the bot page", () => evaluate(`/\\/bots\\/\\d+/.test(location.pathname) && !document.getElementById("bots_scope")`));
  console.log("ok a tile opens its bot page");
  console.log(`screenshots: ${SHOTS}`);
} catch (e) { failures.push(e.stack || String(e)); }
if (errors.length) failures.push(`console errors: ${errors.join(" | ")}`);
browser.kill();
if (failures.length) { console.error(failures.join("\n")); process.exit(1); }
process.exit(0);
"#####;
