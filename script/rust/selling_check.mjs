// The bot page's Sell links while a sale runs (bot--selling, D-LIQUIDATION-ONE-LINE), in a real browser.
//   bun script/rust/selling_check.mjs
// Run by rust/tests/selling_browser.rs. Needs Chrome ($CHROME, else the usual places) and the compiled
// bundle (script/rust/build_assets.sh). The page is a fixture with the markup rust/src/web/figure/holdings.rs
// and web/bot/live.rs::selling_html render (their own tests pin it), served from this process with the
// real app/assets/builds/application.js; headless Chrome is driven over its DevTools protocol.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const root = join(import.meta.dir, "../..");
const bundle = join(root, "app/assets/builds/application.js");
const css = join(root, "app/assets/builds/application.css");
if (!existsSync(bundle) || !existsSync(css)) {
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
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// --- the fixture app ---------------------------------------------------------------------------

const state = (blocked, assets) =>
  `<div id="selling_state_1" hidden data-bot--selling-target="state" data-assets="${JSON.stringify(assets)}" data-blocked="${blocked}" data-label="Selling…"></div>`;
const link = (asset, symbol) =>
  `<a class="rbutton rbutton--small rbutton--secondary" data-bot--selling-target="sell" data-asset="${asset}" data-turbo-frame="modal" href="/bots/1/liquidation/new?asset_id=${asset}&amp;symbol=${symbol}">Sell</a>`;
const metrics = (mark) => `<div id="metrics" data-mark="${mark}"><table><tbody>
  <tr><td>BTC</td><td class="table__action">${link(5, "BTC")}</td></tr>
  <tr><td>ETH</td><td class="table__action">${link(6, "ETH")}</td></tr>
  <tr><td>GONE</td><td class="table__action">${link(7, "GONE")}</td></tr>
</tbody></table>
<a id="sell_all" class="rbutton rbutton--small rbutton--danger" data-bot--selling-target="sell" data-turbo-frame="modal" href="/bots/1/liquidation/new?asset_id%5B%5D=5&amp;symbol%5B%5D=BTC">Sell all</a>
</div>`;
const page = `<!DOCTYPE html>
<html lang="en"><head>
<meta charset="utf-8"><title>Fixture</title>
<meta name="csrf-token" content="fixture"><meta name="turbo-prefetch" content="false">
<link rel="stylesheet" href="/assets/application.css">
<script type="module" src="/assets/application.js"></script>
</head>
<body>
<div id="flash" data-turbo-permanent></div>
<turbo-frame id="modal"></turbo-frame>
<main><div id="columns_1" class="columns" data-controller="bot--selling">
${state(false, [])}
<div class="column">${metrics(0)}</div>
</div></main>
</body></html>`;

const requests = [];
const html = (body) => new Response(body, { headers: { "content-type": "text/html; charset=utf-8" } });
const app = Bun.serve({
  port: 0,
  hostname: "127.0.0.1",
  fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/assets/application.js") return new Response(Bun.file(bundle), { headers: { "content-type": "text/javascript" } });
    if (url.pathname === "/assets/application.css") return new Response(Bun.file(css), { headers: { "content-type": "text/css" } });
    if (url.pathname === "/bots/1") return html(page);
    if (url.pathname === "/bots/1/liquidation/new") {
      requests.push(url.search);
      // web/bot/liquidation.rs::new: a position no longer held answers its flash from inside the modal frame.
      if (url.searchParams.get("symbol") === "GONE") {
        return html(`<html><body><turbo-frame id="modal"><turbo-stream action="prepend" target="flash"><template><div class="salert">GONE is no longer held.</div></template></turbo-stream></turbo-frame></body></html>`);
      }
      return html(`<html><body><turbo-frame id="modal"><div id="confirm">Sell ${url.searchParams.get("symbol")}?</div></turbo-frame></body></html>`);
    }
    return new Response("", { status: 404 });
  },
});
const BASE = `http://127.0.0.1:${app.port}`;

// --- the browser -------------------------------------------------------------------------------

const profile = mkdtempSync(join(tmpdir(), "deltabadger-selling-"));
const browser = spawn(chrome, ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run",
  "--no-default-browser-check", "--window-size=1280,900", "about:blank"], { stdio: "ignore" });
const watchdog = setTimeout(() => { console.error("deadline"); browser.kill("SIGKILL"); process.exit(1); }, 90000);

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
  await send("Page.enable");
  await send("Page.navigate", { url: `${BASE}/bots/1` });
  await until("Stimulus on the page", () => evaluate(`!!window.Turbo && !!document.querySelector("#columns_1") && document.readyState === "complete"`));
  await sleep(300);

  // What a person sees of each Sell link.
  const look = () => evaluate(`[...document.querySelectorAll('[data-bot--selling-target="sell"]')].map((a) => ({
    asset: a.dataset.asset ?? null, inert: a.getAttribute("aria-disabled") === "true", tabindex: a.getAttribute("tabindex"),
    busy: a.parentElement.classList.contains("table__busy"), spinner: a.parentElement.querySelector(".loader--small")?.getAttribute("aria-label") ?? null,
    pointer: getComputedStyle(a).pointerEvents }))`);
  const stream = (action, target, content) => evaluate(`Turbo.renderStreamMessage(${JSON.stringify(`<turbo-stream action="${action}" target="${target}"><template>${content}</template></turbo-stream>`)}); new Promise((r) => setTimeout(r, 100))`);
  const click = async (selector) => {
    const box = await evaluate(`(() => { const r = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()`);
    for (const type of ["mousePressed", "mouseReleased"]) await send("Input.dispatchMouseEvent", { type, x: box.x, y: box.y, button: "left", clickCount: 1 });
    await sleep(400);
  };
  const enter = async (selector) => {
    await evaluate(`document.querySelector(${JSON.stringify(selector)}).focus()`);
    for (const type of ["keyDown", "keyUp"]) await send("Input.dispatchKeyEvent", { type, key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, text: type === "keyDown" ? "\r" : undefined });
    await sleep(400);
  };

  let links = await look();
  check("nothing selling: every link is live", links.length === 4 && links.every((l) => !l.inert && !l.busy && l.tabindex === null), links);

  await stream("replace", "selling_state_1", state(true, [5]));
  links = await look();
  check("BTC selling: its link is inert and wears the spinner", links[0].inert && links[0].busy && links[0].spinner === "Selling…" && links[0].pointer === "none" && links[0].tabindex === "-1", links[0]);
  check("every other link is inert, without a spinner", links.slice(1).every((l) => l.inert && !l.busy), links);

  await click('[data-asset="6"]');
  check("a click on an inert link opens nothing", requests.length === 0 && !(await evaluate(`!!document.querySelector("#confirm")`)), requests);
  await enter('[data-asset="6"]');
  check("Enter on an inert link opens nothing", requests.length === 0, requests);

  await stream("replace", "metrics", metrics(1));
  links = await look();
  check("a panel published mid-sale is inert at once", (await evaluate(`document.querySelector("#metrics").dataset.mark`)) === "1" && links[0].busy && links.every((l) => l.inert), links);

  await stream("replace", "selling_state_1", state(false, []));
  links = await look();
  check("at the unblock every link is live again, spinner gone", links.every((l) => !l.inert && !l.busy && l.tabindex === null && l.pointer !== "none"), links);

  await click('[data-asset="6"]');
  await until("the confirmation", () => evaluate(`!!document.querySelector("#confirm")`)).catch(() => {});
  check("a live link opens its confirmation", requests.length === 1 && (await evaluate(`document.querySelector("#confirm")?.textContent`)) === "Sell ETH?", requests);

  await click('[data-asset="7"]');
  const flash = await until("the flash", () => evaluate(`document.querySelector("#flash")?.textContent.trim()`)).catch(() => "");
  check("a position no longer held answers with a flash", flash === "GONE is no longer held.", flash);

  check("no console errors", errors.length === 0, errors);
} catch (error) {
  console.error(error);
  failures.push(String(error));
} finally {
  clearTimeout(watchdog);
  socket?.close();
  browser.kill("SIGKILL");
  app.stop(true);
  await sleep(200);
  rmSync(profile, { recursive: true, force: true });
}
if (failures.length) {
  console.error(`${failures.length} failed`);
  process.exit(1);
}
console.log("all passed");
