// The first page after sign-in, in a real browser, against a running `deltabadger serve`.
//   BASE_URL=http://127.0.0.1:3999 EMAIL=... PASSWORD=... bun script/rust/browser_check.mjs
// It is run by rust/tests/serve.rs (`cargo test --test serve -- --ignored`), which starts the server.
// Chrome is taken from $CHROME, else from the usual places. No dependencies: headless Chrome is
// driven over its DevTools protocol with bun's own WebSocket and fetch.
// It checks what the page-parity harness cannot see: that the compiled JS and CSS leave the page
// visible after sign-in, and that the real @rails/actioncable client connects once, receives the
// server's pings, and keeps that one connection. With UNPORTED_VENUE (an exchange id), it also picks that venue in the index
// wizard's venue step, where the refusal must show in the modal. With INDEX_WIZARD ({ venue, key, secret, btc }) and BOT_ID, it
// walks the index wizard in the sheet opened over that bot's page (D-INDEX-IN-ASSETS): the index picked with the arrow keys
// from a filtered list, a correct key on to the quote step inside the sheet, and a quote search after the market data went
// away showing its connect form in the sheet; then (PLAN index-as-ingredient slice 6) an index beside BTC, removed and
// picked again, saved as one grouped basket. With GROUP_SHEET ({ bot, group }), a grouped basket's index sheet (PLAN
// index-as-ingredient slice 5): opened by Enter on the stack, the count saved 3 → 4 → 3 (each read back from the stored row),
// a save refused after the bot was started behind the open sheet (the refusal in the sheet, nothing stored), the composition's
// list toggled by keyboard, and the focus back on the stack once the sheet closes.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const { BASE_URL, EMAIL, PASSWORD, BOT_ID, UNPORTED_VENUE, TYPING, INDEX_WIZARD, GROUP_SHEET } = process.env;
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
  const port = await until("Chrome's debugging port", () => { try { return existsSync(portFile) && readFileSync(portFile, "utf8").split("\n")[0]; } catch { return false; } });
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
  // A fixed function called with its arguments as values: nothing is built into code from data.
  const call = async (fn, ...args) => {
    const page = (await send("Runtime.evaluate", { expression: "window" })).result?.result?.objectId;
    return (await send("Runtime.callFunctionOn", { objectId: page, functionDeclaration: fn.toString(), arguments: args.map((value) => ({ value })), returnByValue: true, awaitPromise: true })).result?.result?.value;
  };

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
  await call(function (email, password) {
    document.querySelector("#user_email").value = email;
    document.querySelector("#user_password").value = password;
    document.querySelector("#user_password").form.requestSubmit();
  }, EMAIL, PASSWORD);
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
  // TYPING: [{ bot, field, keys, saved }]. Keys are pressed one by one, slower than the form's 250 ms debounce, as a
  // person types: every value the form submits ("0" is invalid) is answered, and the field keeps what was typed and
  // the focus through each answer, so the next key lands after the last one. "ArrowLeft" moves the caret, so a saved
  // amount can be edited in the middle ("15" into "1.5"). `saved` is the amount the last answer, a 200, draws back.
  for (const { bot, field, keys, saved } of JSON.parse(TYPING || "[]")) {
    await send("Page.navigate", { url: `${BASE_URL}/bots/${bot}` });
    await until(`bot ${bot}'s ${field}`, () => call(function (path, id) { return location.pathname === path && !!document.getElementById(id); }, `/bots/${bot}`, field));
    // Each answer: its status, and the amount its stream draws the field with.
    await call(function (id) {
      window.answers = [];
      document.addEventListener("turbo:submit-end", async (event) => {
        const response = event.detail.fetchResponse;
        const page = new DOMParser().parseFromString(await response.response.clone().text(), "text/html");
        const drawn = [...page.querySelectorAll("template")].map((template) => template.content.getElementById(id)).find(Boolean);
        window.answers.push({ status: response.statusCode, value: drawn?.getAttribute("value") ?? null });
      });
      const input = document.getElementById(id); input.value = ""; input.focus();
    }, field);
    let typed = "";
    for (const key of keys) {
      if (key === "ArrowLeft") {
        for (const type of ["keyDown", "keyUp"]) await send("Input.dispatchKeyEvent", { type, key, code: key, windowsVirtualKeyCode: 37 });
        typed = null; // from here the field is checked by what was saved
      } else {
        await send("Input.insertText", { text: key });
        if (typed !== null) typed += key;
      }
      await sleep(700);
      const look = await call(function (id) {
        const input = document.getElementById(id);
        return { value: input?.value, focused: document.activeElement === input, answers: window.answers.slice() };
      }, field);
      console.log(JSON.stringify({ bot, field, key, ...look }));
      // Chrome reads "0." as "0": a separator just typed shows on the screen, not in the value.
      const kept = typed === null || look.value === typed || look.value === typed.replace(/[.,]$/, "");
      if (!kept || !look.focused) throw new Error(`typing ${keys.join(" ")} into ${field}: after ${key} the field shows ${JSON.stringify(look.value)} (typed ${JSON.stringify(typed)}), focused: ${look.focused}`);
    }
    const last = await evaluate("window.answers.at(-1)");
    const value = await call(function (id) { return document.getElementById(id).value; }, field);
    if (last?.status !== 200 || last.value !== saved || value !== saved) {
      throw new Error(`typing ${keys.join(" ")} into ${field}: the last answer was ${JSON.stringify(last)} and the field shows ${JSON.stringify(value)}, not a 200 saving ${saved}`);
    }
  }
  if (UNPORTED_VENUE) {
    // The venue's button submits the nested #exchange-picker form into #modal_content; the POST redirects to the key step,
    // whose refusal (a venue this build does not trade) must show in the modal, never Turbo's "Content missing".
    await send("Page.navigate", { url: `${BASE_URL}/bots` });
    await until("the bots page", () => evaluate(`location.pathname === "/bots" && !!document.querySelector("turbo-frame#modal")`));
    await evaluate(`document.querySelector("turbo-frame#modal").src = "/bots/new"`);
    await until("the wizard's modal", () => evaluate(`!!document.querySelector("turbo-frame#modal_content")`));
    // The chooser begins a new wizard, so the index is picked after it, as a person picks it.
    const picked = (await send("Runtime.evaluate", { awaitPromise: true, returnByValue: true, expression: `fetch("/bots/dca_indexes/pick_index", {
      method: "POST", headers: { "X-CSRF-Token": document.querySelector('meta[name="csrf-token"]').content,
      "Content-Type": "application/x-www-form-urlencoded", Accept: "text/vnd.turbo-stream.html, text/html" },
      body: "index_type=top&index_category_id=&index_name=" }).then((response) => response.status)` })).result?.result?.value;
    if (picked >= 400) throw new Error(`picking Top coins answered ${picked}`);
    await evaluate(`document.querySelector("turbo-frame#modal_content").src = "/bots/dca_indexes/pick_exchange/new"`);
    const button = `#exchange-picker button[value="${UNPORTED_VENUE}"]`;
    await until("the venue step", () => call(function (sel) { return !!document.querySelector(sel); }, button));
    await call(function (sel) { document.querySelector(sel).click(); }, button);
    await until("the refusal in the modal", () => evaluate(`(document.querySelector("turbo-frame#modal_content")?.innerText ?? "").includes("Not available yet")`));
    if (await evaluate(`document.body.innerText.includes("Content missing")`)) throw new Error("the page shows Turbo's Content missing");
    console.log("unported venue: the key step's refusal shows in the wizard's modal");
  }
  if (GROUP_SHEET) {
    const { bot, group, unbalanced } = JSON.parse(GROUP_SHEET);
    // Weights whose drawn values sum to 100.0 % (33.3 + 33.3 + 33.4) but whose stored sum is 100.12 %: Normalize stays offered.
    await send("Page.navigate", { url: `${BASE_URL}/bots/${unbalanced}` });
    await until("the unbalanced bot's page", () => call(function (p) { return location.pathname === p && !!document.querySelector(".asset-allocations__normalize"); }, `/bots/${unbalanced}`));
    await sleep(500);
    if (await call(function () { return document.querySelector(".asset-allocations__normalize").hidden; })) throw new Error("Normalize hidden for a stored sum of 100.12 %");
    const page = `/bots/${bot}`;
    const stack = `#index_group_${group}_stack`;
    const shows = (selector) => call(function (sel) { return !!document.querySelector(sel); }, selector);
    const field = (name) => `bots_dca_multi_asset[index_groups][${group}][${name}]`;
    const press = async (key, code) => { for (const type of ["keyDown", "keyUp"]) await send("Input.dispatchKeyEvent", { type, key, code: key, windowsVirtualKeyCode: code, ...(type === "keyDown" && key === "Enter" ? { text: "\r" } : {}) }); };
    const focus = (selector) => call(function (sel) { const element = document.querySelector(sel); if (!element) return false; element.focus(); return document.activeElement === element; }, selector);
    // The count the stored row holds, as a fresh sheet draws it.
    const stored = () => call(function (url, name) {
      return fetch(url).then((r) => r.text()).then((html) => new DOMParser().parseFromString(html, "text/html").querySelector(`input[type=range][name="${name}"]`)?.value);
    }, `${page}/index_groups/${group}/edit`, field("num_coins"));
    const save = (count) => call(function (name, value) {
      window.groupSubmission = null;
      const range = document.querySelector(`dialog.sheet-dialog input[type=range][name="${name}"]`);
      range.value = value;
      range.dispatchEvent(new Event("input", { bubbles: true }));
      range.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    }, field("num_coins"), String(count));
    await send("Page.navigate", { url: BASE_URL + page });
    await until("the grouped bot's page", () => call(function (p, sel) { return location.pathname === p && !!document.querySelector(sel); }, page, stack));
    await evaluate(`document.addEventListener("turbo:submit-end", event => { window.groupSubmission = event.detail.fetchResponse?.statusCode; })`);
    // The composition's list, by keyboard.
    const toggle = ".index-composition button[data-action='donut-chart#toggle']";
    if (!(await focus(toggle))) throw new Error("the composition's toggle takes no focus");
    const listHidden = () => call(function () { return document.querySelector(".index-composition [data-donut-chart-target=list]")?.classList.contains("is-hidden"); });
    const before = await listHidden();
    await press("Enter", 13);
    await until("the composition toggled by Enter", async () => (await listHidden()) !== before);
    // Enter on the stack opens the sheet.
    if (!(await focus(stack))) throw new Error("the stack takes no focus");
    await press("Enter", 13);
    await until("the index sheet", () => call(function (name) { return !!document.querySelector(`dialog.sheet-dialog[open] input[type=range][name="${name}"]`); }, field("num_coins")));
    for (const count of [4, 3]) {
      await save(count);
      await until(`count ${count} saved`, () => call(function () { return window.groupSubmission === 200; }));
      await until(`the sheet redrawn at ${count}`, () => call(function (name, value) { return document.querySelector(`dialog.sheet-dialog[open] input[name="${name}"]`)?.value === value; }, field("num_coins_rendered"), String(count)));
      if (await stored() !== String(count)) throw new Error(`count ${count} not stored: ${await stored()}`);
    }
    // An arrow key on the real range saves, redraws the sheet, and the range keeps the focus (its stable id).
    const rangeId = `bots_dca_multi_asset_index_groups_${group}_num_coins`;
    if (!(await focus(`#${rangeId}`))) throw new Error("the count range takes no focus");
    await evaluate(`window.groupSubmission = null`);
    await press("ArrowRight", 39);
    await until("the arrow key's save", () => call(function () { return window.groupSubmission === 200; }));
    await until("the sheet redrawn at 4", () => call(function (name) { return document.querySelector(`dialog.sheet-dialog[open] input[name="${name}"]`)?.value === "4"; }, field("num_coins_rendered")));
    await until("the focus kept on the range", () => call(function (id) { return document.activeElement?.id === id; }, rangeId));
    if (await stored() !== "4") throw new Error(`the arrow key stored ${await stored()}`);
    await save(3);
    await until("count 3 saved again", () => call(function () { return window.groupSubmission === 200; }));
    if (await stored() !== "3") throw new Error(`count 3 not stored again: ${await stored()}`);
    // Started behind the open sheet: its next save is refused inside the sheet, and nothing is stored.
    const started = await call(function (url) {
      const body = new URLSearchParams({ _method: "patch" });
      return fetch(url, { method: "POST", body, headers: { "X-CSRF-Token": document.querySelector('meta[name="csrf-token"]').content, Accept: "text/vnd.turbo-stream.html" } }).then((r) => r.status);
    }, `${page}/start`);
    if (started !== 200) throw new Error(`starting the grouped bot answered ${started}`);
    await save(4);
    await until("the refused save", () => call(function () { return window.groupSubmission === 422; }));
    await until("the refusal in the sheet", () => shows("dialog.sheet-dialog[open] .form__info--invalid"));
    if (await stored() !== "3") throw new Error(`a refused save stored ${await stored()}`);
    // Closed: the focus is back on the stack.
    await evaluate(`document.querySelector("dialog.sheet-dialog .sheet__close").click()`);
    await until("the sheet closed", () => call(function () { return !document.querySelector("dialog.sheet-dialog[open]"); }));
    await until("the focus back on the stack", () => call(function (sel) { return document.activeElement === document.querySelector(sel); }, stack));
    console.log("index sheet: Normalize on the stored sum, Enter opens it, count 3 → 4 → 3 stored, an arrow key keeps the range's focus, a raced refusal in the sheet, the list by keyboard, focus back on the stack");
  }
  if (INDEX_WIZARD && BOT_ID) {
    const { venue, key, secret, btc } = JSON.parse(INDEX_WIZARD);
    const page = `/bots/${BOT_ID}`;
    const frame = "turbo-frame#modal_content";
    const shows = (selector) => call(function (sel) { return !!document.querySelector(sel); }, selector);
    const click = async (selector) => {
      if (!(await call(function (sel) { const element = document.querySelector(sel); if (!element) return false; element.click(); return true; }, selector))) throw new Error(`missing control: ${selector}`);
    };
    const press = async (key, code) => { for (const type of ["keyDown", "keyUp"]) await send("Input.dispatchKeyEvent", { type, key, code: key, windowsVirtualKeyCode: code }); };
    const type = async (selector, text) => {
      await call(function (sel) { const input = document.querySelector(sel); input.value = ""; input.focus(); }, selector);
      await send("Input.insertText", { text });
    };
    const inSheet = async (what) => {
      await until(what, () => call(function (p, f) { return location.pathname === p && !!document.querySelector(`div.sheet ${f} .sheet__track`); }, page, frame));
      if (await evaluate(`document.body.innerText.includes("Content missing")`)) throw new Error(`${what}: Turbo's Content missing`);
    };
    const market = (provider) => call(function (value) {
      const body = new URLSearchParams({ _method: "patch", market_data_provider: value });
      return fetch("/settings/update_market_data", { method: "POST", body, headers: { "X-CSRF-Token": document.querySelector('meta[name="csrf-token"]').content, Accept: "text/vnd.turbo-stream.html" } }).then((r) => r.status);
    }, provider);
    await send("Page.navigate", { url: BASE_URL + page });
    await until("the bot page", () => call(function (p) { return location.pathname === p && !!document.querySelector("turbo-frame#modal"); }, page));
    // New bot: no chooser, the wizard's first step in the sheet over the bot's page.
    await evaluate(`document.querySelector("turbo-frame#modal").src = "/bots/new"`);
    await inSheet("the wizard's first step");
    await click(`${frame} .sheet__track form[action$="/bots/dca_single_assets/order"] button`);
    await until("the Asset view", () => shows(`${frame} .wizard-search .segmented a[href$="kind=index"]`));
    await click(`${frame} .wizard-search .segmented a[href$="kind=index"]`);
    await until("the Index view", () => shows(`${frame} button.index-row`));
    // The Asset / Index switch folds only when it does not fit its row (the search box beside it yields first): open on the
    // desktop-wide sheet, though it was first measured while the sheet zoomed in; open at a phone's 375 px, where it still
    // fits; folded on a row narrower than itself; open again once the room is back.
    const folded = () => call(function (f) { return document.querySelector(`${f} .wizard-search .segmented`)?.classList.contains("segmented--collapsed"); }, frame);
    await sleep(800);
    if (await folded()) throw new Error("the Asset / Index switch folded on a desktop-wide sheet");
    const width = (px) => send("Emulation.setDeviceMetricsOverride", { width: px, height: 800, deviceScaleFactor: 1, mobile: false });
    await width(375);
    await sleep(800);
    if (await folded()) throw new Error("the Asset / Index switch folded at 375 px, where it fits");
    await width(150);
    await until("the switch folded on a row narrower than itself", () => folded());
    await send("Emulation.clearDeviceMetricsOverride");
    await until("the switch open again at the desktop width", async () => (await folded()) === false);
    // Enter on the focused switch is the link's own: it never picks an index.
    await call(function (f) { document.querySelector(`${f} .wizard-search .segmented a[href$="kind=index"]`).focus(); }, frame);
    await press("Enter", 13);
    await sleep(1500);
    if (!(await shows(`${frame} button.index-row`)) || (await shows(`${frame} #exchange-picker`))) throw new Error("Enter on the switch picked an index");
    // A filter that matches nothing: Enter picks nothing.
    await type(`${frame} input[data-index-filter-target=input]`, "zzzz");
    await sleep(300);
    await press("Enter", 13);
    await sleep(1500);
    if (!(await shows(`${frame} button.index-row`))) throw new Error("Enter with no match left the Index view");
    // Filtered to one: the arrows and Enter pick the visible row, never a hidden one.
    await type(`${frame} input[data-index-filter-target=input]`, "layer");
    await sleep(300);
    await press("ArrowDown", 40);
    await press("Enter", 13);
    // D-INDEX-JOINS-BASKET: the pick joins the selection and the step stays; its row is listed and Next goes on.
    await until("the pick in the selection", () => shows(`${frame} .wizard-assets__list input[name=index_row]`));
    const picked = await evaluate(`document.querySelector("${frame} .sheet__track li").innerText`);
    if (!/Layer/.test(picked)) throw new Error(`the arrow keys picked ${JSON.stringify(picked)}, not the filtered Layer 1`);
    await click(`${frame} form.sheet__next button`);
    await until("the index exchange step", () => shows(`${frame} #exchange-picker button[value="${venue}"]`));
    await inSheet("the index exchange step");
    await click(`${frame} #exchange-picker button[value="${venue}"]`);
    await until("the key step", () => shows(`${frame} form[action*="/bots/dca_indexes/add_api_key"] input[name="api_key[key]"]`));
    await call(function (f, k, s) {
      const form = document.querySelector(`${f} form[action*="/bots/dca_indexes/add_api_key"]`);
      form.querySelector('input[name="api_key[key]"]').value = k;
      form.querySelector('input[name="api_key[secret]"]').value = s;
      form.requestSubmit();
    }, frame, key, secret);
    await until("the quote step", () => shows(`${frame} .sheet__search form[action$="/bots/dca_indexes/pick_spendable_asset/new"]`));
    await inSheet("the quote step, the bot page still under the sheet");
    // The market data goes away; a search on the quote step, then on the exchange step, shows the connect form in the sheet.
    if (await market("") !== 200) throw new Error("switching market data off");
    await type(`${frame} .sheet__search input[name=query]`, "us");
    await until("the connect form after a quote search", () => shows(`${frame} a.exchange-grid__item--provider`));
    await inSheet("the connect form after a quote search");
    if (await market("deltabadger") !== 200) throw new Error("switching market data back on");
    await click(`${frame} .sheet__track a[href$="/bots/dca_indexes/pick_exchange/new"]`);
    await until("the index exchange step again", () => shows(`${frame} #exchange-picker`));
    // The exchange step has no search (D-EXCHANGE-NO-SEARCH), so the quote step's search is the one left to check.
    await evaluate(`document.querySelector("dialog.sheet-dialog .sheet__close").click()`);
    await until("the sheet closed", () => call(function () { return !document.querySelector("dialog.sheet-dialog[open]"); }));
    console.log("index wizard: arrow keys on the filtered list, the key on to the quote in the sheet, the connect form after a search");
    // PLAN index-as-ingredient slice 6: a fresh wizard, BTC in the Asset view, then Top beside it; the track shows the chip and
    // one stack; Top's × removes it and a second pick brings it back; Next, the venue both offer, the key it has, the quote,
    // and the bot page draws Top as a group row.
    const trackStacks = () => call(function (f) { return document.querySelectorAll(`${f} .sheet__track .sheet__label--tickers .ticker-group--many`).length; }, frame);
    const trackChip = () => call(function (f) { return !!document.querySelector(`${f} .sheet__track [data-ticker-symbol=BTC]`); }, frame);
    await send("Page.navigate", { url: BASE_URL + page });
    await until("the bot page again", () => call(function (p) { return location.pathname === p && !!document.querySelector("turbo-frame#modal"); }, page));
    await evaluate(`document.querySelector("turbo-frame#modal").src = "/bots/new"`);
    await inSheet("a new wizard");
    await click(`${frame} .sheet__track form[action$="/bots/dca_single_assets/order"] button`);
    await until("the Asset view", () => shows(`${frame} button[name="bots_dca_single_asset[base_asset_id]"][value="${btc}"]`));
    await click(`${frame} button[name="bots_dca_single_asset[base_asset_id]"][value="${btc}"]`);
    await until("BTC chosen", () => trackChip());
    await click(`${frame} .wizard-search .segmented a[href$="kind=index"]`);
    await until("the Index view", () => shows(`${frame} button.index-row[data-index-category-id="top-coins"]`));
    await click(`${frame} button.index-row[data-index-category-id="top-coins"]`);
    await until("BTC and one stack", async () => (await trackChip()) && (await trackStacks()) === 1);
    await click(`${frame} .wizard-assets__list form:has(input[name=index_row]) button`);
    await until("Top removed", async () => (await trackChip()) && (await trackStacks()) === 0);
    // A removal comes back to the Asset view with the basket open, as an asset's does; the switch leads to the Index view.
    await click(`${frame} .wizard-search .segmented a[href$="kind=index"]`);
    await until("Top offered again", () => shows(`${frame} button.index-row[data-index-category-id="top-coins"]`));
    await click(`${frame} button.index-row[data-index-category-id="top-coins"]`);
    await until("Top back", async () => (await trackStacks()) === 1);
    await click(`${frame} form.sheet__next button`);
    await until("the venues both offer", () => shows(`${frame} #exchange-picker button[value="${venue}"]`));
    await inSheet("the mixed exchange step");
    if (await trackStacks() !== 1) throw new Error("the exchange step's track lost the stack");
    await click(`${frame} #exchange-picker button[value="${venue}"]`);
    await until("the mixed quote step", () => shows(`${frame} button[name="bots_dca_multi_asset[quote_asset_id]"]`));
    await click(`${frame} button[name="bots_dca_multi_asset[quote_asset_id]"]`);
    await until("the grouped bot's page", () => call(function () { return /^\/bots\/\d+$/.test(location.pathname) && !!document.querySelector("#index_group_top-coins_stack"); }));
    console.log("mixed wizard: an index beside BTC, removed and re-added, saved as one grouped basket");
  }
  if (BOT_ID) {
    const path = `/bots/${BOT_ID}`;
    const quotedPath = JSON.stringify(path);
    const columns = `#columns_bots_dca_multi_asset_${BOT_ID}`;
    const amount = "#bots_dca_multi_asset_quote_amount";
    const click = async (selector) => {
      const clicked = await call(function (sel) { const element = document.querySelector(sel); if (!element) return false; element.click(); return true; }, selector);
      if (!clicked) throw new Error(`missing control: ${selector}`);
    };
    await send("Page.navigate", { url: BASE_URL + path });
    await until("bot settings", () => call(function (p, sel) { return location.pathname === p && !!document.querySelector(sel); }, path, amount));
    // The field is parsed before the module scripts run: an input event dispatched then finds no submit-after-delay
    // controller on its form and submits nothing. Wait for Stimulus to have connected it.
    await until("the amount form's submit controller", () => call(function (sel) {
      const form = document.querySelector(sel)?.closest("form");
      return !!form && !!window.Stimulus?.getControllerForElementAndIdentifier(form, "form--submit-after-delay");
    }, amount));
    await evaluate(`document.addEventListener("turbo:submit-end", event => { window.lastSubmission = event.detail.fetchResponse?.statusCode; })`);
    const edit = async (value, status) => {
      await call(function (sel, v) { window.lastSubmission = null; const field = document.querySelector(sel); field.value = v; field.dispatchEvent(new Event("input", { bubbles: true })); }, amount, value);
      await until(`amount ${value} response ${status}`, () => call(function (s) { return window.lastSubmission === s; }, status));
      await until(`amount ${value} remains in form`, () => call(function (sel, v, p) { return document.querySelector(sel)?.value === v && location.pathname === p; }, amount, value, path));
    };
    await edit("7", 200);
    await edit("0", 422);
    await until("inline amount error", () => call(function (sel) { return !!document.querySelector(sel + ".is-invalid") && !!document.querySelector("#settings .form__info--invalid"); }, amount));
    await edit("7", 200);
    await until("cleared inline error", () => call(function (sel) { return !document.querySelector(sel + ".is-invalid"); }, amount));

    // A separate page target shares only the browser session, not the first tab's DOM.
    const tab = await (await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(BASE_URL + path)}`, { method: "PUT" })).json();
    const second = new WebSocket(tab.webSocketDebuggerUrl);
    socketsToClose.push(second);
    await new Promise((resolve, reject) => { second.onopen = resolve; second.onerror = reject; });
    let secondId = 0;
    const pending = new Map();
    second.onmessage = event => { const message = JSON.parse(event.data); const settle = pending.get(message.id); if (typeof settle === "function") settle(message); };
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
  // Windows keeps the killed browser's profile locked for a moment: retry, and never fail a run on a leftover temp folder.
  try { rmSync(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 }); } catch (error) { console.warn(`profile not removed: ${error.code ?? error}`); }
}
if (problem) {
  console.error(problem);
  process.exit(1);
}
console.log("browser check passed");
