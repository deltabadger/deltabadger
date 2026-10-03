// The first page after sign-in, in a real browser, against a running `deltabadger serve`.
//   BASE_URL=http://127.0.0.1:3999 EMAIL=... PASSWORD=... bun script/rust/browser_check.mjs
// It is run by rust/tests/serve.rs (`cargo test --test serve -- --ignored`), which starts the server.
// Chrome is taken from $CHROME, else from the usual places. No dependencies: headless Chrome is
// driven over its DevTools protocol with bun's own WebSocket and fetch.
// It checks what the page-parity harness cannot see: that the compiled JS and CSS leave the page
// visible after sign-in, and that the real @rails/actioncable client connects once, receives the
// server's pings, and keeps that one connection.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const { BASE_URL, EMAIL, PASSWORD, BOT_ID } = process.env;
if (!BASE_URL || !EMAIL || !PASSWORD) {
  console.error("set BASE_URL, EMAIL and PASSWORD");
  process.exit(2);
}
const places = [
  process.env.CHROME,
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/Applications/Chromium.app/Contents/MacOS/Chromium",
  "/usr/bin/google-chrome",
  "/usr/bin/google-chrome-stable",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
].filter(Boolean);
const chrome = places.find((path) => existsSync(path));
if (!chrome) {
  console.error(`no Chrome found: set CHROME to its binary (looked in ${places.join(", ")})`);
  process.exit(2);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what, check, ms = 20000) {
  const end = Date.now() + ms;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > end || Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await sleep(100);
  }
}

const profile = mkdtempSync(join(tmpdir(), "deltabadger-browser-check-"));
const browser = spawn(
  chrome,
  ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--window-size=1280,900", "about:blank"],
  { stdio: "ignore" },
);

// What the page looks like to a person: is the navbar there, is the content there, are the streams live.
const LOOK = `(() => {
  const shown = (selector) => [...document.querySelectorAll(selector)].some((element) => element.getClientRects().length > 0);
  const sources = [...document.querySelectorAll("turbo-cable-stream-source")];
  return {
    path: location.pathname,
    hideChrome: document.body.classList.contains("hide-chrome"),
    navbar: shown("body > .menu, body > .menu-mobile, body > .hamburger"),
    content: shown("body > main.main .main-content"),
    text: (document.querySelector(".main-content")?.innerText ?? "").trim().length,
    modalSrc: document.querySelector("turbo-frame#modal")?.getAttribute("src") ?? null,
    sources: sources.length,
    connected: sources.filter((element) => element.hasAttribute("connected")).length,
  };
})()`;

const deadline = Date.now() + 120000;
const watchdog = setTimeout(() => browser.kill("SIGKILL"), 120000);
const socketsToClose = [];
let problem = null;
const complaints = []; // errors the browser logged: failed requests, refused scripts
// Every WebSocket the page opens to /cable, as the browser's network layer reports it: when it was
// created, when each ping frame arrived, and whether it was closed.
const cables = new Map();
let where = async () => "";
try {
  const portFile = join(profile, "DevToolsActivePort");
  const port = await until("Chrome's debugging port", () => existsSync(portFile) && readFileSync(portFile, "utf8").split("\n")[0]);
  const target = await until("a page to drive", async () => {
    const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    return targets.find((candidate) => candidate.type === "page");
  });
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  socketsToClose.push(socket);
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = () => reject(new Error("cannot reach Chrome's DevTools socket"));
  });
  let id = 0;
  const waiting = new Map();
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.method === "Log.entryAdded" && message.params.entry.level === "error") complaints.push(`${message.params.entry.text} ${message.params.entry.url ?? ""}`);
    const frames = message.params && cables.get(message.params.requestId);
    if (message.method === "Network.webSocketCreated" && new URL(message.params.url).pathname === "/cable") cables.set(message.params.requestId, { pings: [], closed: false });
    if (message.method === "Network.webSocketFrameReceived" && frames && message.params.response.payloadData.includes('"type":"ping"')) frames.pings.push(Date.now());
    if (message.method === "Network.webSocketClosed" && frames) frames.closed = true;
    if (waiting.has(message.id)) {
      waiting.get(message.id)(message);
      waiting.delete(message.id);
    }
  };
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const requestId = ++id;
      const timer = setTimeout(() => { waiting.delete(requestId); reject(new Error(`DevTools timeout: ${method}`)); }, 5000);
      waiting.set(requestId, (message) => { clearTimeout(timer); resolve(message); });
      socket.send(JSON.stringify({ id: requestId, method, params }));
    });
  // `undefined` while a navigation is replacing the document: callers wait with `until`.
  const evaluate = async (expression) => (await send("Runtime.evaluate", { expression, returnByValue: true })).result?.result?.value;

  // For a failure message: where the browser is and what it shows.
  where = async () => `\n  at ${await evaluate("location.href")}\n  showing: ${JSON.stringify(await evaluate("document.body.innerText.slice(0, 300)"))}`;
  await send("Log.enable");
  await send("Network.enable");

  await send("Page.navigate", { url: `${BASE_URL}/login` });
  await until("the login form", () => evaluate(`!!document.querySelector("#user_email") && !!document.querySelector("#user_password")`));
  // Submitted at once, on purpose: the browser's own background request for the web manifest's
  // start_url ("/") is then still in flight with the signed-out cookie. When the session cookie was
  // written on every response, its late answer put the signed-out session back and this sign-in was
  // lost. The cookie is now written only when the session changed (web::session).
  await evaluate(`(() => {
    document.querySelector("#user_email").value = ${JSON.stringify(EMAIL)};
    document.querySelector("#user_password").value = ${JSON.stringify(PASSWORD)};
    document.querySelector("#user_password").form.requestSubmit();
  })()`);
  await until("the bots page after signing in", () => evaluate(`location.pathname === "/bots" && !!document.querySelector("main.main")`));

  const first = await until("every stream source to connect", async () => {
    const look = await evaluate(LOOK);
    return look && look.sources > 0 && look.connected === look.sources && look;
  });
  // @rails/actioncable calls a connection stale when 6 seconds pass without a ping, and looks for
  // that every 6 to 12 seconds (connection_monitor.js: staleThreshold, getPollInterval). A look at
  // the page after some seconds could miss a reconnect that had already finished, so the frames
  // themselves are watched, for 13 seconds: one connection, never closed, and a ping at least every
  // 6 seconds on it.
  const WATCH = 13000;
  const watchedFrom = Date.now();
  await sleep(WATCH);
  const later = await evaluate(LOOK);

  const wrong = [];
  const sockets = [...cables.values()];
  if (sockets.length !== 1) wrong.push(`the page opened ${sockets.length} connections to /cable, not 1: it reconnected, or never connected`);
  for (const { pings, closed } of sockets) {
    if (closed) wrong.push("a connection to /cable was closed");
    const times = [watchedFrom, ...pings.filter((at) => at >= watchedFrom), watchedFrom + WATCH];
    const longest = Math.max(...times.slice(1).map((at, index) => at - times[index]));
    if (pings.length < 4 || longest > 6000) wrong.push(`${pings.length} pings arrived, the longest wait for one was ${longest} ms: the client would call this connection stale`);
  }
  for (const [when, look] of [["after signing in", first], ["13 seconds later", later]]) {
    if (look.path !== "/bots") wrong.push(`${when}: the page is ${look.path}, not /bots`);
    if (look.hideChrome) wrong.push(`${when}: <body> has hide-chrome, which hides the navbar and the content`);
    if (!look.navbar) wrong.push(`${when}: no navbar is visible`);
    if (!look.content || look.text === 0) wrong.push(`${when}: the main content is not visible`);
    if (look.modalSrc) wrong.push(`${when}: the modal frame loads ${look.modalSrc}`);
    if (look.sources < 2 || look.connected !== look.sources) wrong.push(`${when}: ${look.connected} of ${look.sources} stream sources are connected`);
  }
  console.log(JSON.stringify({ first, later, cable: sockets.map(({ pings, closed }) => ({ pings: pings.length, closed })) }));
  if (wrong.length) throw new Error(wrong.join("\n"));
  if (BOT_ID) {
    const path = `/bots/${BOT_ID}`;
    const quotedPath = JSON.stringify(path);
    const columns = `#columns_bots_dca_multi_asset_${BOT_ID}`;
    const amount = "#bots_dca_multi_asset_quote_amount";
    const click = async (selector) => {
      const clicked = await evaluate(`(() => { const element = document.querySelector(${JSON.stringify(selector)}); if (!element) return false; element.click(); return true; })()`);
      if (!clicked) throw new Error(`missing control: ${selector}`);
    };
    await send("Page.navigate", { url: BASE_URL + path });
    await until("bot settings", () => evaluate(`location.pathname === ${quotedPath} && !!document.querySelector(${JSON.stringify(amount)})`));
    await evaluate(`document.addEventListener("turbo:submit-end", event => { window.lastSubmission = event.detail.fetchResponse?.statusCode; })`);
    const edit = async (value, status) => {
      await evaluate(`(() => { window.lastSubmission = null; const field = document.querySelector(${JSON.stringify(amount)}); field.value = ${JSON.stringify(value)}; field.dispatchEvent(new Event("input", { bubbles: true })); })()`);
      await until(`amount ${value} response ${status}`, () => evaluate(`window.lastSubmission === ${status}`));
      await until(`amount ${value} remains in form`, () => evaluate(`document.querySelector(${JSON.stringify(amount)})?.value === ${JSON.stringify(value)} && location.pathname === ${quotedPath}`));
    };
    await edit("7", 200);
    await edit("0", 422);
    await until("inline amount error", () => evaluate(`!!document.querySelector("${amount}.is-invalid") && !!document.querySelector("#settings .form__info--invalid")`));
    await edit("7", 200);
    await until("cleared inline error", () => evaluate(`!document.querySelector("${amount}.is-invalid")`));

    // A separate page target shares only the browser session, not the first tab's DOM.
    const tab = await (await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(BASE_URL + path)}`, { method: "PUT" })).json();
    const second = new WebSocket(tab.webSocketDebuggerUrl);
    socketsToClose.push(second);
    await new Promise((resolve, reject) => { second.onopen = resolve; second.onerror = reject; });
    let secondId = 0;
    const pending = new Map();
    second.onmessage = event => { const message = JSON.parse(event.data); pending.get(message.id)?.(message); };
    const secondEvaluate = expression => new Promise((resolve, reject) => {
      const id = ++secondId;
      const timer = setTimeout(() => { pending.delete(id); reject(new Error("second-tab DevTools timeout")); }, 5000);
      pending.set(id, message => { clearTimeout(timer); pending.delete(id); resolve(message.result?.result?.value); });
      second.send(JSON.stringify({ id, method: "Runtime.evaluate", params: { expression, returnByValue: true } }));
    });
    await until("second tab subscribed", () => secondEvaluate(`location.pathname === ${quotedPath} && [...document.querySelectorAll("turbo-cable-stream-source")].filter(s => s.hasAttribute("connected")).length >= 2`));
    await click(`form[action="${path}/start/edit"] button`);
    await until("restart modal with both choices", () => evaluate(`location.pathname === ${quotedPath} && !!document.querySelector('#modal dialog[open]') && !!document.querySelector('#modal form[action$="start_fresh=true"]') && !!document.querySelector('#modal form[action$="start_fresh=false"]')`));
    await click('#modal form[action$="start_fresh=false"] button');
    const working = `document.querySelector(${JSON.stringify(columns)})?.classList.contains("bot-locked") && !!document.querySelector('form[action="${path}/stop"]')`;
    await until("working first tab", () => evaluate(working));
    await until("working status and column lock in second tab", () => secondEvaluate(working));
    await until("restart modal closed", () => evaluate(`!document.querySelector('#modal dialog[open]') && location.pathname === ${quotedPath}`));
    await click(`form[action="${path}/stop"] button`);
    const stopped = `!!document.querySelector(${JSON.stringify(columns)}) && !document.querySelector(${JSON.stringify(columns)}).classList.contains("bot-locked") && !document.querySelector('form[action="${path}/stop"]')`;
    await until("stopped first tab", () => evaluate(stopped));
    await until("stopped and unlocked second tab", () => secondEvaluate(stopped));
    await click(`a[href="${path}/archive/edit"]`);
    await until("archive confirmation frame", () => evaluate(`!!document.querySelector('#modal dialog[open] form[action="${path}/archive"]') && location.pathname === ${quotedPath}`));
    await click(`#modal form[action="${path}/archive"] button`);
    await until("reactivate button after archive", () => evaluate(`!!document.querySelector('#status_button_bots_dca_multi_asset_${BOT_ID} form[action="${path}/archive"] input[value="delete"]')`));
    await until("archive confirmation closed", () => evaluate(`!document.querySelector('#modal dialog[open]')`));
    await click(`#status_button_bots_dca_multi_asset_${BOT_ID} form[action="${path}/archive"] button`);
    await until("reactivated page", () => evaluate(`location.pathname === ${quotedPath} && !document.querySelector('#status_button_bots_dca_multi_asset_${BOT_ID} form[action="${path}/archive"]') && !!document.querySelector('a[href="${path}/archive/edit"]')`));
    await click(`a[href="${path}/delete/edit"]`);
    await until("delete confirmation frame", () => evaluate(`!!document.querySelector('#modal dialog[open] form[action="${path}/delete"]') && location.pathname === ${quotedPath}`));
    await click(`#modal form[action="${path}/delete"] button`);
    await until("deleted bot returns to list", () => evaluate(`location.pathname === "/bots" && !document.querySelector('#status_button_bots_dca_multi_asset_${BOT_ID}') && !document.querySelector('#modal dialog[open]')`));
    console.log("bot actions: amount, inline error, restart choices, two-tab status/locks, stop, archive, reactivate, delete passed");
  }
} catch (error) {
  problem = String(error?.message ?? error) + (await where().catch(() => "")) + (complaints.length ? `\n  the browser logged:\n    ${complaints.join("\n    ")}` : "");
} finally {
  clearTimeout(watchdog);
  for (const socket of socketsToClose) socket.close();
  const exited = new Promise(resolve => { if (browser.exitCode !== null || browser.signalCode !== null) resolve(); else browser.once("exit", resolve); });
  browser.kill("SIGKILL");
  await exited;
  rmSync(profile, { recursive: true, force: true });
}
if (problem) {
  console.error(problem);
  process.exit(1);
}
console.log("browser check passed");
