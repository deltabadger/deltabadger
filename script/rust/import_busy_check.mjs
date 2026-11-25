// A fresh tracker upload answered busy, then the same file chosen again, in a real browser (PLAN-710-importmem C3,
// D-IMPORT-BUSY): the import modal as the app renders it (MODAL), the busy answer the app writes (BUSY) and the compiled
// bundle's file-picker. The busy answer must leave the drop zone as it was before the file was chosen, its line shown,
// so that choosing the very same file again posts it again.
//   MODAL=modal.html BUSY=busy.html FILE=orders.csv bun script/rust/import_busy_check.mjs
// Run by rust/tests/import_busy_browser.rs. Needs Chrome ($CHROME, else the usual places) and the compiled bundle
// (script/rust/build_assets.sh). No dependencies: the page is a fixture served from this process. Exits 77 when no
// Chrome is found.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const root = join(import.meta.dir, "../..");
const bundle = join(root, "rust/web/builds/application.js");
const css = join(root, "rust/web/builds/application.css");
const { MODAL, BUSY, FILE } = process.env;
if (!MODAL || !BUSY || !FILE) { console.error("set MODAL, BUSY and FILE"); process.exit(2); }
if (!existsSync(bundle) || !existsSync(css)) { console.error("no rust/web/builds/application.{js,css}: run script/rust/build_assets.sh"); process.exit(2); }
// Windows keeps Chrome under Program Files (either one) or the user's own LOCALAPPDATA.
const windows = [process.env.PROGRAMFILES, process.env["PROGRAMFILES(X86)"], process.env.LOCALAPPDATA].filter(Boolean)
  .map((base) => join(base, "Google", "Chrome", "Application", "chrome.exe"));
const chrome = [
  process.env.CHROME,
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/Applications/Chromium.app/Contents/MacOS/Chromium",
  "/usr/bin/google-chrome",
  "/usr/bin/google-chrome-stable",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
  ...windows,
].filter(Boolean).find((path) => existsSync(path));
// 77: no browser here, which the Rust test reports as skipped.
if (!chrome) { console.error("no Chrome found: set CHROME to its binary"); process.exit(77); }
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// --- the fixture app ---------------------------------------------------------------------------

const page = `<!DOCTYPE html>
<html lang="en"><head>
<meta charset="utf-8"><title>Fixture</title>
<meta name="csrf-token" content="fixture"><meta name="turbo-prefetch" content="false">
<link rel="stylesheet" href="/assets/application.css">
<script type="module" src="/assets/application.js"></script>
</head>
<body>
<div id="flash" data-turbo-permanent></div>
<turbo-frame id="modal_content">${readFileSync(MODAL, "utf8")}</turbo-frame>
</body></html>`;
const posts = [];
const app = Bun.serve({
  port: 0,
  hostname: "127.0.0.1",
  async fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/assets/application.js") return new Response(Bun.file(bundle), { headers: { "content-type": "text/javascript" } });
    if (url.pathname === "/assets/application.css") return new Response(Bun.file(css), { headers: { "content-type": "text/css" } });
    if (url.pathname === "/tracker") return new Response(page, { headers: { "content-type": "text/html; charset=utf-8" } });
    if (url.pathname === "/tracker/import" && request.method === "POST") {
      const form = await request.formData();
      posts.push(form.get("file")?.name ?? null);
      // The first post is busy, as the app answers it; the retry is written.
      if (posts.length === 1) return new Response(readFileSync(BUSY, "utf8"), { status: 503, headers: { "content-type": "text/vnd.turbo-stream.html; charset=utf-8", "retry-after": "5" } });
      return new Response(`<turbo-stream action="append" target="flash"><template><p id="written">Imported</p></template></turbo-stream>`,
        { headers: { "content-type": "text/vnd.turbo-stream.html; charset=utf-8" } });
    }
    return new Response("", { status: 404 });
  },
});
const BASE = `http://127.0.0.1:${app.port}`;

// --- the browser -------------------------------------------------------------------------------

const profile = mkdtempSync(join(tmpdir(), "deltabadger-import-busy-"));
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
  // A file chosen in the drop zone's input, as the picker sets it: `change` fires only when the choice differs from the
  // input's value.
  const choose = async () => {
    const document = await send("DOM.getDocument", { depth: -1, pierce: true });
    const input = await send("DOM.querySelector", { nodeId: document.result.root.nodeId, selector: "input#file" });
    await send("DOM.setFileInputFiles", { files: [FILE], nodeId: input.result.nodeId });
  };
  await send("Runtime.enable");
  await send("Page.enable");
  await send("DOM.enable");
  await send("Page.navigate", { url: `${BASE}/tracker` });
  await until("Stimulus on the page", () => evaluate(`!!window.Turbo && !!document.querySelector("label.dropzone") && document.readyState === "complete"`));
  await sleep(300);
  const look = () => evaluate(`(() => { const zone = document.querySelector("label.dropzone"); const input = document.querySelector("input#file");
    return { busy: zone.classList.contains("is-busy"), filled: zone.classList.contains("has-file"), value: input.value,
             line: document.querySelector("#import_busy")?.textContent ?? null, lines: document.querySelectorAll("#import_busy").length }; })()`);

  await choose();
  await until("the first post", () => posts.length === 1);
  const busy = await until("the busy line", async () => { const l = await look(); return l.line ? l : null; });
  check("busy: the drop zone is idle again, its input empty, the line shown once", !busy.busy && !busy.filled && busy.value === "" && busy.lines === 1, busy);

  await choose();
  await until("the retry", () => posts.length === 2).catch(() => {});
  check("the same file chosen again is posted again", posts.length === 2 && posts[1] === posts[0], posts);
  const written = await until("the written answer", () => evaluate(`!!document.querySelector("#written")`)).catch(() => false);
  check("the retry is answered", written, posts);
  check("no exceptions", errors.length === 0, errors);
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
