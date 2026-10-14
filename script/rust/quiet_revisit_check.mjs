// A revisit of a bot page in a real browser: what quiet_revisit.js, the chart controller and the
// ticker tooltips do across Turbo's preview and the server's answer.
//   bun script/rust/quiet_revisit_check.mjs
// Run by rust/tests/quiet_revisit_browser.rs. Needs Chrome ($CHROME, else the usual places) and the
// compiled bundle (script/rust/build_assets.sh). No dependencies: the pages are fixtures served from
// this process, with the real app/assets/builds/application.js and a minimal Action Cable endpoint,
// and headless Chrome is driven over its DevTools protocol.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const root = join(import.meta.dir, "../..");
const bundle = join(root, "app/assets/builds/application.js");
if (!existsSync(bundle) || !existsSync(join(root, "app/assets/builds/application.css"))) {
  console.error("no app/assets/builds/application.{js,css}: run script/rust/build_assets.sh");
  process.exit(2);
}
const chrome = [
  process.env.CHROME,
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/Applications/Chromium.app/Contents/MacOS/Chromium",
  "/usr/bin/google-chrome",
  "/usr/bin/google-chrome-stable",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
].filter(Boolean).find((path) => existsSync(path));
if (!chrome) {
  console.error("no Chrome found: set CHROME to its binary");
  process.exit(2);
}

// --- the fixture app ---------------------------------------------------------------------------

// What the server answers, changed by the checks as they go.
const server = {
  chart: { 1: "inline", 2: "inline" }, // "inline", or "lazy" (a frame to load)
  version: { 1: 0, 2: 0 },             // moves the figures, so the answer differs from the preview
  // ms before a bot page answers. Never under a frame or two: an answer that beats the next paint
  // makes Turbo drop the preview it was about to show, and a revisit with no preview is not one.
  delay: { 1: 150, 2: 150 },
  tooltipDelay: 0,
  requests: [],
};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const escape = (text) => String(text).replaceAll("&", "&amp;").replaceAll('"', "&quot;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");

function figures(id) {
  const start = Date.UTC(2026, 0, 1);
  const labels = [], value = [], invested = [], pnl = [];
  for (let day = 0; day < 40; day++) {
    labels.push(new Date(start + day * 86400000).toISOString());
    invested.push(100 + day * 10);
    value.push(100 + day * 10 + Math.sin(day / 3) * 20 + server.version[id] * 5 + id);
    pnl.push(value[day] / invested[day] - 1);
  }
  return { labels, series: [value, invested], pnl };
}

const segmented = `<div class="segmented segmented--fluid" role="radiogroup" aria-label="Chart" data-controller="segmented" data-action="click->segmented#toggle" data-segmented-key="chart-mode">
  <span class="segmented__thumb" data-segmented-target="thumb"></span>
  <div class="segmented__dropdown" data-segmented-target="dropdown"><div class="segmented__panes"><div class="segmented__pane segmented__menu">
    <button type="button" class="segmented__option is-on" role="radio" aria-checked="true" tabindex="0" data-value="pnl" data-segmented-target="option" data-action="segmented#select keydown->segmented#keys">P/L</button>
    <button type="button" class="segmented__option" role="radio" aria-checked="false" tabindex="-1" data-value="value" data-segmented-target="option" data-action="segmented#select keydown->segmented#keys">Value</button>
  </div></div></div>
  <span class="segmented__caret" aria-hidden="true"></span>
</div>`;

function chart(id) {
  const f = figures(id);
  const data = { bot: id, quote: "USDC", decimals: 2, labels: f.labels, series: f.series, pnl: f.pnl, assets: {}, "pnl-only": false, buys: [], prices: {}, "buy-logos": {} };
  const attributes = Object.entries(data).map(([key, value]) => ` data-bot--chart-${key}-value="${escape(typeof value === "string" ? value : JSON.stringify(value))}"`).join("");
  return `<div id="chart" class="widget widget--chart">
  <div data-controller="bot--chart" data-action="turbo:before-stream-render@document->bot--chart#restore"${attributes}>
    <div class="widget--chart__head">
      <div class="widget--chart__modes" data-action="segmented:change->bot--chart#mode">${segmented}</div>
      <div class="widget--chart__summary" data-bot--chart-target="summary">
        <div class="widget--chart__date" data-bot--chart-target="date">&nbsp;</div>
        <div class="widget--chart__pnl" data-bot--chart-target="pnl">&nbsp;</div>
        <div class="widget--chart__percent" data-bot--chart-target="percent">&nbsp;</div>
      </div>
    </div>
    <div class="widget--chart__plot" style="width: 900px; height: 300px;">
      <canvas data-bot--chart-target="analyzerChart" width="10" height="2"></canvas>
      <div class="widget--chart__buys" data-bot--chart-target="buys"></div>
    </div>
    <div class="widget--chart__axis" data-bot--chart-target="axis"></div>
    <div class="widget--chart__orders">
      <input type="checkbox" data-bot--chart-target="orders" data-action="change->bot--chart#toggleOrders">
      <input type="checkbox" data-bot--chart-target="prices" data-action="change->bot--chart#togglePrices">
    </div>
  </div>
</div>`;
}

const placeholder = `<div id="chart" class="widget widget--chart"><div><div class="loader"></div></div></div>`;
const chartFrame = (id) => server.chart[id] === "inline"
  ? `<turbo-frame id="bot_chart">${chart(id)}</turbo-frame>`
  : `<turbo-frame id="bot_chart" src="/bots/${id}/chart">${placeholder}</turbo-frame>`;

const layout = (body) => `<!DOCTYPE html>
<html lang="en"><head>
<meta charset="utf-8"><title>Fixture</title>
<meta name="csrf-token" content="fixture"><meta name="turbo-prefetch" content="false">
<link rel="stylesheet" href="/assets/application.css">
<script type="module" src="/assets/application.js"></script>
</head>
<body data-controller="ticker-tooltips">
<div id="flash" data-turbo-permanent></div>
<main>${body}</main>
</body></html>`;

const botPage = (id) => layout(`
<a id="to-bots" href="/bots">Bots</a>
<turbo-frame id="bot">
  <turbo-cable-stream-source channel="Turbo::StreamsChannel" signed-stream-name="bot_${id}_page"></turbo-cable-stream-source>
  <a class="switch" href="/bots/${id === 1 ? 2 : 1}" data-turbo-frame="bot" data-turbo-action="advance">Other bot</a>
  <span class="ticker" data-ticker-symbol="BTC">BTC</span>
  ${chartFrame(id)}
  <div id="columns_bot_${id}" class="columns"></div>
</turbo-frame>`);

const botsPage = () => layout(`<a href="/bots/1">One</a> <a href="/bots/2">Two</a>
  <turbo-cable-stream-source channel="Turbo::StreamsChannel" signed-stream-name="user_bots"></turbo-cable-stream-source>`);

// Action Cable as @rails/actioncable expects it: a welcome, confirmed subscriptions, pings.
const cables = new Set();
const subscriptions = []; // every subscribe the browser ever sent: [stream, at]
const html = (body) => new Response(body, { headers: { "content-type": "text/html; charset=utf-8" } });
const app = Bun.serve({
  port: 0,
  hostname: "127.0.0.1",
  async fetch(request, bun) {
    const url = new URL(request.url);
    server.requests.push(url.pathname + url.search);
    if (url.pathname === "/cable") return bun.upgrade(request) ? undefined : new Response("no", { status: 400 });
    if (url.pathname === "/assets/application.js") return new Response(Bun.file(bundle), { headers: { "content-type": "text/javascript" } });
    // The colours the chart reads are the stylesheet's custom properties.
    if (url.pathname === "/assets/application.css") return new Response(Bun.file(join(root, "app/assets/builds/application.css")), { headers: { "content-type": "text/css" } });
    if (url.pathname.startsWith("/assets/")) return new Response("", { status: 404 });
    if (url.pathname === "/bots") return html(botsPage());
    if (url.pathname === "/asset_tooltip") {
      await sleep(server.tooltipDelay);
      return html(`<div class="tooltip--ticker__col">Bitcoin</div>`);
    }
    let match = url.pathname.match(/^\/bots\/(\d)\/chart$/);
    if (match) return html(layout(`<turbo-frame id="bot_chart">${chart(Number(match[1]))}</turbo-frame>`));
    match = url.pathname.match(/^\/bots\/(\d)$/);
    if (match) {
      const id = Number(match[1]);
      await sleep(server.delay[id]);
      return html(botPage(id));
    }
    return new Response("", { status: 404 });
  },
  websocket: {
    open(ws) {
      cables.add(ws);
      ws.send(JSON.stringify({ type: "welcome" }));
      ws.data = setInterval(() => ws.send(JSON.stringify({ type: "ping", message: Math.floor(Date.now() / 1000) })), 3000);
    },
    message(ws, raw) {
      const message = JSON.parse(raw);
      if (message.command !== "subscribe") return;
      subscriptions.push([JSON.parse(message.identifier).signed_stream_name, Date.now()]);
      ws.send(JSON.stringify({ identifier: message.identifier, type: "confirm_subscription" }));
    },
    close(ws) {
      clearInterval(ws.data);
      cables.delete(ws);
    },
  },
});
function broadcast(stream, payload) {
  const identifier = JSON.stringify({ channel: "Turbo::StreamsChannel", signed_stream_name: stream });
  for (const ws of cables) ws.send(JSON.stringify({ identifier, message: payload }));
}
const BASE = `http://127.0.0.1:${app.port}`;

// --- the browser -------------------------------------------------------------------------------

const profile = mkdtempSync(join(tmpdir(), "deltabadger-quiet-revisit-"));
const browser = spawn(chrome, ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run",
  "--no-default-browser-check", "--window-size=1280,900", "about:blank"], { stdio: "ignore" });
const watchdog = setTimeout(() => { console.error("deadline"); browser.kill("SIGKILL"); process.exit(1); }, 150000);

async function until(what, check, ms = 10000) {
  const end = Date.now() + ms;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(50);
  }
}

const failures = [];
function check(name, condition, detail) {
  console.log(`${condition ? "ok  " : "FAIL"} ${name}${condition ? "" : `: ${JSON.stringify(detail)}`}`);
  if (!condition) failures.push(name);
}

let socket;
const errors = [];
try {
  const portFile = join(profile, "DevToolsActivePort");
  const port = await until("Chrome's debugging port", () => existsSync(portFile) && readFileSync(portFile, "utf8").split("\n")[0], 20000);
  const target = await until("a page", async () => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === "page"));
  socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = () => reject(new Error("no DevTools socket")); });
  let id = 0;
  const waiting = new Map();
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    // Stimulus reports a controller's error through console.error rather than throwing it.
    if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") errors.push(message.params.args.map((a) => a.description ?? a.value).join(" "));
    if (message.method === "Runtime.exceptionThrown") errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text);
    if (waiting.has(message.id)) { waiting.get(message.id)(message); waiting.delete(message.id); }
  };
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const requestId = ++id;
    const timer = setTimeout(() => { waiting.delete(requestId); reject(new Error(`DevTools timeout: ${method}`)); }, 15000);
    waiting.set(requestId, (message) => { clearTimeout(timer); resolve(message); });
    socket.send(JSON.stringify({ id: requestId, method, params }));
  });
  const evaluate = async (expression) => {
    const answer = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (answer.result?.exceptionDetails) throw new Error(`${expression.slice(0, 80)}: ${answer.result.exceptionDetails.exception?.description}`);
    return answer.result?.result?.value;
  };
  await send("Runtime.enable");

  // Helpers inside the page: a visit and its end, marks on nodes (properties, never attributes, so
  // the marks themselves never make a node differ from the server's), and a reading of the page.
  const HELPERS = `
    // A chart is constructed on a canvas it asks for a context (Chart.js asks twice); an update in
    // place asks for none. So the canvases asked are the charts built.
    window.__built = 0;
    window.__asked = new WeakSet();
    const getContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function (...args) {
      if (this.matches('[data-bot--chart-target="analyzerChart"]') && !__asked.has(this)) { __asked.add(this); window.__built++; }
      return getContext.apply(this, args);
    };
    window.__renders = [];
    addEventListener("turbo:render", () => __renders.push(location.pathname + (document.documentElement.hasAttribute("data-turbo-preview") ? " preview" : "")));
    window.__loaded = () => new Promise((resolve) => addEventListener("turbo:load", resolve, { once: true }));
    window.__visit = async (path) => { const loaded = __loaded(); Turbo.visit(path); await loaded; await new Promise((r) => setTimeout(r, 300)); };
    window.__onPreview = (fn) => { const run = () => { if (document.documentElement.hasAttribute("data-turbo-preview")) { removeEventListener("turbo:render", run); fn(); } }; addEventListener("turbo:render", run); };
    window.__chart = () => { const el = document.querySelector('[data-controller~="bot--chart"]'); return el && Stimulus.getControllerForElementAndIdentifier(el, "bot--chart"); };
    window.__read = () => {
      const chart = __chart();
      return {
        path: location.pathname,
        bot: document.querySelector("turbo-frame#bot .columns[id]")?.id ?? null,
        botMark: document.querySelector("turbo-frame#bot")?.__mark ?? null,
        canvasMark: document.querySelector("turbo-frame#bot_chart canvas")?.__mark ?? null,
        frameSrc: document.querySelector("turbo-frame#bot_chart")?.getAttribute("src") ?? null,
        drawn: !!chart?.chart,
        mode: chart?.currentMode ?? null,
        switchOn: document.querySelector(".widget--chart__modes .is-on")?.dataset.value ?? null,
        datasets: chart?.chart?.data.datasets.length ?? null,
        streams: [...document.querySelectorAll("turbo-cable-stream-source")].map((s) => ({ mark: s.__mark ?? null, connected: s.hasAttribute("connected") })),
        flash: document.querySelectorAll("#flash").length,
        tooltips: document.querySelectorAll(".tooltip--ticker").length,
        built: window.__built,
        renders: __renders.slice(-4),
      };
    };`;
  const load = async (path) => {
    await send("Page.navigate", { url: BASE + path });
    await until(`${path} to load`, () => evaluate(`typeof Turbo !== "undefined" && typeof Stimulus !== "undefined" && document.readyState === "complete"`).catch(() => false));
    await evaluate(HELPERS);
    await sleep(300);
  };
  const read = () => evaluate("__read()");
  const chartRequests = (id) => server.requests.filter((path) => path.startsWith(`/bots/${id}/chart`)).length;
  // bots → the bot, marking what the preview drew so the answer can be checked against it.
  const revisit = async (id) => {
    await evaluate(`__visit("/bots")`);
    await evaluate(`window.__built = 0`);
    await evaluate(`__onPreview(() => {
      const canvas = document.querySelector("turbo-frame#bot_chart canvas"); if (canvas) canvas.__mark = "preview";
      document.querySelector("turbo-frame#bot").__mark = "preview";
      document.querySelectorAll("turbo-cable-stream-source").forEach((s, i) => { s.__mark = "preview" + i; });
    })`);
    await evaluate(`__visit("/bots/${id}")`);
    return read();
  };

  // 1. The revisit morphs: the preview's chart stays, nothing is fetched or built again.
  await load("/bots/1");
  await until("the stream to connect", () => evaluate(`[...document.querySelectorAll("turbo-cable-stream-source")].every((s) => s.hasAttribute("connected"))`));
  let before = chartRequests(1);
  server.version[1]++;
  let page = await revisit(1);
  check("revisit keeps the preview's chart", page.canvasMark === "preview" && page.drawn, page);
  check("revisit builds the chart once (the preview's)", page.built === 1, page);
  check("revisit fetches no chart frame", chartRequests(1) === before, server.requests.slice(-5));
  check("revisit leaves one permanent #flash", page.flash === 1, page);
  // The last point of the plotted curve is the answer's, not the preview's (one version behind).
  const plotted = await evaluate(`__chart().chart.data.datasets[0].data.at(-1).y`);
  const answered = figures(1).pnl.at(-1);
  server.version[1]--;
  const previewed = figures(1).pnl.at(-1);
  server.version[1]++;
  check("revisit draws the answer's figures in place", Math.abs(plotted - answered) < 1e-9 && Math.abs(plotted - previewed) > 1e-6, { plotted, answered, previewed });

  // 2. Stream subscriptions live through the morph, and still deliver.
  check("revisit keeps connected stream sources", page.streams.length === 1 && page.streams[0].mark === "preview0" && page.streams[0].connected, page.streams);
  const subscribed = subscriptions.filter(([stream]) => stream === "bot_1_page").length;
  await sleep(300);
  check("revisit does not subscribe again", subscriptions.filter(([stream]) => stream === "bot_1_page").length === subscribed, subscriptions);
  broadcast("bot_1_page", `<turbo-stream action="update" target="columns_bot_1"><template><p id="delivered">yes</p></template></turbo-stream>`);
  check("a broadcast after the revisit lands", await until("the broadcast", () => evaluate(`!!document.getElementById("delivered")`)).catch(() => false), null);

  // 3. The chosen mode survives: the switch and the plot agree after a revisit.
  await evaluate(`document.querySelector('.widget--chart__modes [data-value="value"]').click()`);
  await sleep(200);
  page = await revisit(1);
  check("revisit in VALUE keeps a VALUE plot under a VALUE switch", page.mode === "value" && page.switchOn === "value" && page.canvasMark === "preview", page);
  await evaluate(`document.querySelector('.widget--chart__modes [data-value="pnl"]').click()`);
  await sleep(200);

  // 4. A navigation that overtakes the preview is never morphed into it.
  server.delay[1] = 1500;
  await evaluate(`__visit("/bots")`);
  await evaluate(`__onPreview(() => { document.querySelector("turbo-frame#bot").__mark = "preview-of-1"; Turbo.visit("/bots/2"); })`);
  await evaluate(`(async () => { const done = new Promise((r) => { const f = () => { if (location.pathname === "/bots/2") { removeEventListener("turbo:load", f); r(); } }; addEventListener("turbo:load", f); }); Turbo.visit("/bots/1"); await done; await new Promise((r) => setTimeout(r, 300)); })()`);
  page = await read();
  check("an overtaking visit renders its own page, not a morph of the preview", page.bot === "columns_bot_2" && page.botMark === null, page);
  server.delay[1] = 150;
  await sleep(1600);
  page = await read();
  check("the overtaken answer never lands", page.bot === "columns_bot_2", page);
  // Overtaken by a bot that is cached: that bot's own preview, then its answer morphed into it.
  await evaluate(`__visit("/bots/2")`);
  server.delay[1] = 1500;
  server.delay[2] = 600;
  await evaluate(`__visit("/bots")`);
  await evaluate(`__onPreview(() => {
    __onPreview(() => { document.querySelector("turbo-frame#bot").__mark = "preview-of-2"; const c = document.querySelector("turbo-frame#bot_chart canvas"); if (c) c.__mark = "preview-of-2"; });
    setTimeout(() => Turbo.visit("/bots/2"), 50); // a click on the painted preview
  })`);
  await evaluate(`(async () => { const done = new Promise((r) => { const f = () => { if (location.pathname === "/bots/2") { removeEventListener("turbo:load", f); r(); } }; addEventListener("turbo:load", f); }); Turbo.visit("/bots/1"); await done; await new Promise((r) => setTimeout(r, 300)); })()`);
  page = await read();
  check("overtaken by a cached bot: its answer morphs into its own preview", page.bot === "columns_bot_2" && page.botMark === "preview-of-2" && page.canvasMark === "preview-of-2", page);
  server.delay[1] = 150;
  server.delay[2] = 150;
  await sleep(1600);

  // 5. After the bot switcher #bot carries a src; a revisit still keeps the ready chart.
  await load("/bots/1");
  await evaluate(`(async () => { const loaded = new Promise((r) => addEventListener("turbo:frame-load", r, { once: true })); document.querySelector("a.switch").click(); await loaded; await new Promise((r) => setTimeout(r, 300)); })()`);
  page = await read();
  const switched = await evaluate(`document.querySelector("turbo-frame#bot").getAttribute("src")`);
  check("the switcher leaves a src on #bot", page.bot === "columns_bot_2" && !!switched, { page, switched });
  before = chartRequests(2);
  page = await revisit(2);
  check("after the switcher, a revisit keeps the chart", page.canvasMark === "preview" && page.built === 1 && chartRequests(2) === before, page);

  // 6. A chart that came through its lazy frame, then is ready inline: kept.
  server.chart[1] = "lazy";
  await load("/bots/1");
  await until("the lazy chart", () => evaluate(`!!__chart()?.chart`));
  server.chart[1] = "inline";
  before = chartRequests(1);
  page = await revisit(1);
  check("a lazily loaded chart is kept when the answer has it inline", page.canvasMark === "preview" && page.frameSrc === null && page.built === 1 && chartRequests(1) === before, page);

  // 7. ... but when the answer still has a frame to load, it loads: what loaded before is no proof of now.
  server.chart[1] = "lazy";
  await load("/bots/1");
  await until("the lazy chart", () => evaluate(`!!__chart()?.chart`));
  before = chartRequests(1);
  page = await revisit(1);
  await until("the frame to load again", async () => chartRequests(1) > before, 3000).catch(() => null);
  page = await until("the reloaded chart", async () => { const p = await read(); return p.drawn && p; }).catch(read);
  check("a frame the answer still has to load loads again", chartRequests(1) === before + 1 && page.canvasMark === null, { page, requests: chartRequests(1) - before });
  server.chart[1] = "inline";

  // 8. A pointer held over the plot while new figures arrive: the headline stays on its point.
  await load("/bots/1");
  const box = await evaluate(`(() => { const r = document.querySelector("turbo-frame#bot_chart canvas").getBoundingClientRect(); return { x: r.left + r.width * 0.3, y: r.top + r.height / 2 }; })()`);
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: box.x, y: box.y });
  const hovered = await until("the hovered point", () => evaluate(`(() => { const c = __chart(); return c.chart.tooltip.getActiveElements().length > 0 && document.querySelector('[data-bot--chart-target="date"]').textContent.trim(); })()`));
  await evaluate(`(() => { const el = document.querySelector('[data-controller~="bot--chart"]'); const s = JSON.parse(el.getAttribute("data-bot--chart-series-value")); s[0] = s[0].map((v) => v * 1.1); el.setAttribute("data-bot--chart-series-value", JSON.stringify(s)); })()`);
  await sleep(200);
  const held = await evaluate(`({ date: document.querySelector('[data-bot--chart-target="date"]').textContent.trim(), active: __chart().chart.tooltip.getActiveElements().length })`);
  check("new figures under a held pointer keep the headline on its point", held.active > 0 && held.date === hovered, { hovered, held });
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 5, y: 5 });

  // 9. A tooltip card that arrives after its page left never comes back as an orphan.
  server.tooltipDelay = 800;
  await load("/bots/1");
  const pill = await evaluate(`(() => { const r = document.querySelector(".ticker").getBoundingClientRect(); return { x: r.left + 3, y: r.top + r.height / 2 }; })()`);
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: pill.x, y: pill.y });
  await sleep(100);
  await evaluate(`__visit("/bots")`);
  await sleep(1200);
  page = await read();
  check("a late tooltip card leaves no orphan popover", page.tooltips === 1, page);
  server.tooltipDelay = 0;

  check("no uncaught errors", errors.length === 0, errors);
} catch (error) {
  console.error(error.stack ?? error);
  if (errors.length) console.error(`uncaught in the page: ${errors.join("\n")}`);
  failures.push("harness");
} finally {
  clearTimeout(watchdog);
  try { socket?.close(); } catch {}
  browser.kill("SIGKILL");
  app.stop(true);
  rmSync(profile, { recursive: true, force: true });
}
if (failures.length) {
  console.error(`${failures.length} failed: ${failures.join(", ")}`);
  process.exit(1);
}
console.log("all checks passed");
