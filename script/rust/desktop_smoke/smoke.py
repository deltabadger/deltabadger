#!/usr/bin/env python3
"""The installed desktop app's smoke proofs (PLAN-B6 §9 B6-2, r1.14, r3.4(b), r4.3, r4.5), shared by desktop.yml's legs.

    smoke.py --kind nsis|appimage|deb|macos --app <installed app> --work <empty dir> [--fixture-2x <tar>] <scenario>...

Each scenario runs the installed test build (identifier com.deltabadger.app.test) on its own
DELTABADGER_DESKTOP_DATA_DIR under --work, reads the launcher's `data:` line and asserts it names that dir before it acts,
and on failure prints the scenario's deltabadger.log and deltabadger-stderr.log. What the window shows is read over
WebDriver: tauri-driver (TAURI_DRIVER) on Linux, msedgedriver (MSEDGEDRIVER) on Windows (see Window); macOS has no
WebDriver, so its scenarios read the launcher log and the engine over HTTP. Python 3 standard library only.
"""
import argparse
import hashlib
from http.client import RemoteDisconnected  # `http` below is this script's own helper
import json
import os
import re
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tarfile
import time
import traceback
import urllib.error
import urllib.request
from pathlib import Path

WIN = os.name == "nt"
MAC = sys.platform == "darwin"
LINUX = sys.platform.startswith("linux")
TEST_ID = "com.deltabadger.app.test"
ENGINE = "deltabadger-engine"
REPO = Path(__file__).resolve().parents[3]


class Fail(Exception):
    pass


def say(text):
    print(f"[{time.strftime('%H:%M:%S')}] {text}", flush=True)


def check(ok, text):
    if not ok:
        raise Fail(text)


def until(what, seconds, probe, every=0.3):
    """probe() until it returns something truthy, or fail naming `what` and the deadline."""
    end = time.monotonic() + seconds
    while True:
        got = probe()
        if got:
            return got
        if time.monotonic() > end:
            raise Fail(f"{what}: not within {seconds} s")
        time.sleep(every)


def run(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, **kw)


# ---------- processes ----------

def pids(name):
    """Running processes of the installed app: `Deltabadger` (the shell) or `deltabadger-engine`."""
    if WIN:
        out = run(["tasklist", "/FI", f"IMAGENAME eq {name}.exe", "/FO", "CSV", "/NH"]).stdout
        return [int(row[1]) for row in (line.strip('"').split('","') for line in out.splitlines()) if len(row) > 1 and row[0].lower() == f"{name}.exe".lower()]
    if MAC:
        out = run(["pgrep", "-f", f"{ARGS.app}/Contents/MacOS/{name}( |$)"]).stdout
        return [int(p) for p in out.split()]
    found = []
    for entry in os.listdir("/proc"):
        if entry.isdigit():
            try:
                if os.path.basename(os.readlink(f"/proc/{entry}/exe")) == name:
                    found.append(int(entry))
            except OSError:
                pass  # gone, a zombie, or not ours
    return found


def app_pids():
    return pids("Deltabadger")


def engine_pids():
    return pids(ENGINE)


def kill(pid, hard=True):
    if WIN:
        run(["taskkill", "/F", "/PID", str(pid)])
    else:
        try:
            os.kill(pid, signal.SIGKILL if hard else signal.SIGTERM)
        except ProcessLookupError:
            pass


class ExitWatch:
    """The engine's exit status, where the OS lets a non-parent read it: Windows through a handle opened while it runs;
    Linux because this process is the child subreaper the orphaned engine is reparented to. macOS: not readable (the
    engine writes "stopped on request" right before its exit(0), main.rs)."""

    def __init__(self, pid):
        self.pid, self.handle = pid, None
        if WIN:
            import ctypes
            self.k32 = ctypes.windll.kernel32
            self.handle = self.k32.OpenProcess(0x1000 | 0x00100000, False, pid)  # QUERY_LIMITED_INFORMATION | SYNCHRONIZE

    def wait(self, seconds):
        """The exit code within `seconds` (None where unreadable), or fail."""
        if WIN:
            import ctypes
            check(self.handle, f"no handle on engine {self.pid}")
            check(self.k32.WaitForSingleObject(self.handle, int(seconds * 1000)) == 0, f"engine {self.pid} still running after {seconds} s")
            code = ctypes.c_ulong()
            self.k32.GetExitCodeProcess(self.handle, ctypes.byref(code))
            self.k32.CloseHandle(self.handle)
            return code.value
        if LINUX:
            def reaped():
                try:
                    pid, status = os.waitpid(self.pid, os.WNOHANG)
                except ChildProcessError:
                    return None  # still the app's child (the app not yet reaped by its parent)
                return (os.waitstatus_to_exitcode(status),) if pid else None
            return until(f"engine {self.pid} exit", seconds, reaped)[0]
        until(f"engine {self.pid} exit", seconds, lambda: self.pid not in engine_pids())
        return None


def listening(pid):
    """The local addresses `pid` listens on (TCP)."""
    if WIN:
        out = run(["netstat", "-ano"]).stdout
        return sorted({f[1] for f in (line.split() for line in out.splitlines()) if len(f) == 5 and f[3] == "LISTENING" and f[4] == str(pid)})
    if MAC:
        out = run(["lsof", "-nP", "-a", "-p", str(pid), "-iTCP", "-sTCP:LISTEN"]).stdout
        return sorted({line.split()[8] for line in out.splitlines()[1:]})
    out = run(["ss", "-ltnpH"]).stdout
    return sorted({f.split()[3] for f in out.splitlines() if f"pid={pid}," in f})


def lock_state(data):
    path = data / "db" / ".engine.lock"
    if not path.exists():
        return "absent"
    with open(path, "rb") as f:
        if WIN:
            import msvcrt
            try:
                msvcrt.locking(f.fileno(), msvcrt.LK_NBLCK, 1)
            except OSError:
                return "held"
            msvcrt.locking(f.fileno(), msvcrt.LK_UNLCK, 1)
            return "free"
        import fcntl
        try:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return "held"
        fcntl.flock(f, fcntl.LOCK_UN)
        return "free"


def stamp(path):
    """sha256 and mtime: P8's `.secrets` check."""
    st = path.stat()
    return hashlib.sha256(path.read_bytes()).hexdigest()[:16], st.st_mtime_ns


def tree(root):
    """Every file under `root`: relative path -> (size, sha256)."""
    out = {}
    for path in sorted(Path(root).rglob("*")):
        if path.is_file() and not path.is_symlink():
            out[path.relative_to(root).as_posix()] = (path.stat().st_size, hashlib.sha256(path.read_bytes()).hexdigest())
    return out


def http(port, path, timeout=5):
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=timeout) as r:
            return r.status
    except urllib.error.HTTPError as e:
        return e.code
    except OSError:
        return None


# ---------- the launcher log ----------

def log_of(data):
    return data / "log" / "deltabadger.log"


def lines(data):
    try:
        return log_of(data).read_text(encoding="utf-8", errors="replace").splitlines()
    except FileNotFoundError:
        return []


def starts(data):
    return sum(1 for line in lines(data) if line.startswith("desktop: the engine started on port "))


def since_start(data):
    """The lines after this start's first `--- start <token> ---` marker (the launcher's; the engine's logger writes it too)."""
    all_lines = lines(data)
    tokens = [m.group(1) for m in (re.fullmatch(r"--- start ([0-9a-f]{32}) ---", x) for x in all_lines) if m]
    if not tokens:
        return []
    return all_lines[all_lines.index(f"--- start {tokens[-1]} ---") + 1:]


def logged(data, text):
    """Some line since this start's marker contains `text`."""
    return any(text in x for x in since_start(data))


def check_data_line(data):
    want = f"data: {data} primary: {data / 'db' / 'production.sqlite3'} queue: {data / 'db' / 'production_queue.sqlite3'}"
    got = until("the launcher's data: line", 60, lambda: [x for x in lines(data) if x.startswith("data: ")])[-1]
    check(got == want, f"data line names another dir:\n  got  {got}\n  want {want}")
    say(f"data line: {got}")


class Start:
    def __init__(self, port, pid):
        self.port, self.pid = port, pid


def wait_start(data, before, page="app", seconds=300):
    """A new start (more `engine started` lines than `before`) that shows the engine's `page` (app | refused)."""
    until(f"a new engine start in {log_of(data)}", seconds, lambda: starts(data) > before)
    want = f"desktop: showing the engine's {page} page"

    def shown():
        got = since_start(data)
        stopped = [x for x in got if x.startswith("desktop: showing stopped") or x.startswith("desktop: showing not-started")]
        check(not stopped, f"waiting for the {page} page, the launcher showed: {stopped}")
        return want in got and got
    got = until(want, seconds, shown)
    m = [m for m in (re.fullmatch(r"desktop: the engine started on port (\d+) \(process (\d+)\)", x) for x in got) if m]
    check(m, "no 'engine started' line since this start's marker")
    start = Start(int(m[-1].group(1)), int(m[-1].group(2)))
    say(f"the engine's {page} page is shown (port {start.port}, engine {start.pid})")
    return start


# ---------- launching ----------

def scenario_env(data, **extra):
    env = dict(os.environ, DELTABADGER_DESKTOP_DATA_DIR=str(data))
    env.update(extra)
    return env


def launch(data, **extra):
    """The installed app, started as a user starts it, on `data`."""
    out = open(ARGS.work / f"launch-{data.parent.name}-{time.monotonic_ns()}.out", "wb")
    if MAC:
        envs = [a for k, v in {"DELTABADGER_DESKTOP_DATA_DIR": str(data), **extra}.items() for a in ("--env", f"{k}={v}")]
        return subprocess.Popen(["open", "-n", "-a", str(ARGS.app), *envs], stdout=out, stderr=subprocess.STDOUT)
    return subprocess.Popen([str(ARGS.app)], env=scenario_env(data, **extra), stdout=out, stderr=subprocess.STDOUT)


def the_one(what, found):
    check(len(found) == 1, f"{what}: expected one process, found {found}")
    return found[0]


def stop_hard_and_drain(data, start, what, refused=False):
    """The app killed (taskkill /F, kill -9): the engine, its stdin closed, drains and exits 0 on its own; lock free.
    A refusal page stops without a log line (web/refused.rs `serve` returns 0), so there the exit status is the proof."""
    watch = ExitWatch(start.pid)
    for pid in app_pids():
        kill(pid)
    until("the app gone", 15, lambda: not app_pids())
    code = watch.wait(60)
    check(code == 0 if refused else code in (0, None), f"{what}: engine exit {code}")
    check(refused or logged(data, "stopped on request"), f"{what}: no 'stopped on request' since this start")
    check(lock_state(data) == "free", f"{what}: lock {lock_state(data)}")
    say(f"{what}: app killed; the engine stopped{'' if refused else ' on request'}, exit {code}, lock free")


def mac_quit(data, start, what):
    run(["osascript", "-e", f'quit app id "{TEST_ID}"'])
    until("the app gone", 30, lambda: not app_pids())
    until("the engine gone", 60, lambda: start.pid not in engine_pids())
    got = since_start(data)
    check(any("stopped on request" in x for x in got), f"{what}: no 'stopped on request' since this start")
    check(not [x for x in got if "kill" in x.lower()], f"{what}: a kill line")
    check(lock_state(data) == "free", f"{what}: lock {lock_state(data)}")
    say(f"{what}: app gone, the engine stopped on request (exit status not readable on macOS), lock free, no kill line")


def cleanup():
    """Nothing of the app left running (every scenario ends here, passed or not)."""
    for pid in app_pids():
        if MAC:
            run(["osascript", "-e", f'quit app id "{TEST_ID}"'])
        else:
            kill(pid)
    try:
        until("the app gone", 30, lambda: not app_pids())
        until("the engine gone", 60, lambda: not engine_pids())
    except Fail as e:
        say(f"cleanup: {e}; killing what is left: app {app_pids()} engine {engine_pids()}")
        for pid in app_pids() + engine_pids():
            kill(pid)
        time.sleep(2)


# ---------- the window ----------

class Window:
    """The app's window read over WebDriver (W3C, JSON over HTTP). Linux: tauri-driver starts the app under
    WebKitWebDriver. Windows: the app is started as a user starts it, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS opening
    WebView2's DevTools port (a test build's launcher adds it to wry's arguments, lib.rs `test_webview_args`: WebView2
    itself ignores the variable once an app passes arguments), and msedgedriver attaches to it (`debuggerAddress`)."""

    def __init__(self, data, **extra):
        self.port = free_port()
        self.out = open(ARGS.work / f"webdriver-{data.parent.name}-{time.monotonic_ns()}.out", "wb")
        if WIN:
            devtools = free_port()
            self.app = launch(data, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f"--remote-debugging-port={devtools}", **extra)
            try:
                until("WebView2's DevTools port", 60, lambda: http(devtools, "/json/version") == 200)
            except Fail:
                # Why: did the flag reach WebView2's browser process?
                ps = "Get-CimInstance Win32_Process -Filter \"Name='msedgewebview2.exe'\" | ForEach-Object { $_.ProcessId.ToString() + ' ' + $_.CommandLine }"
                print(run(["powershell", "-NoProfile", "-Command", ps]).stdout[:6000])
                raise
            args = [os.environ["MSEDGEDRIVER"], f"--port={self.port}"]
            caps = {"browserName": "webview2", "ms:edgeOptions": {"debuggerAddress": f"127.0.0.1:{devtools}"}}
            env = None
        else:
            args = [os.environ["TAURI_DRIVER"], "--port", str(self.port), "--native-port", str(free_port())]
            caps = {"tauri:options": {"application": str(ARGS.app)}}
            env = scenario_env(data, **extra)
        self.proc = subprocess.Popen(args, env=env, stdout=self.out, stderr=subprocess.STDOUT)
        until("the WebDriver server answering", 30, lambda: self._status())
        self.sid = self.call("POST", "/session", {"capabilities": {"alwaysMatch": caps}}, timeout=180)["sessionId"]
        say(f"WebDriver session {self.sid[:8]}")

    def _status(self):
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{self.port}/status", timeout=2):
                return True
        except OSError:
            return False

    def call(self, method, path, body=None, timeout=30):
        req = urllib.request.Request(f"http://127.0.0.1:{self.port}{path}", method=method,
                                     data=None if body is None else json.dumps(body).encode(),
                                     headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return json.loads(r.read() or b"{}").get("value")
        except urllib.error.HTTPError as e:
            raise Fail(f"WebDriver {method} {path}: {e.code} {e.read()[:300]!r}")

    def js(self, script, *args, leaves=False):
        """`leaves`: the script starts a navigation (a download included). WebKitWebDriver may close the request's connection
        as the page goes, before it answers: that is the script done, not the driver gone (any later call says if it is)."""
        return self._leaving(leaves, lambda: self.call("POST", f"/session/{self.sid}/execute/sync", {"script": script, "args": list(args)}))

    @staticmethod
    def _leaving(leaves, f):
        try:
            return f()
        except RemoteDisconnected:
            if not leaves:
                raise
            say("the page went before WebDriver answered (it navigated)")
            return None

    def poll(self, what, seconds, script, *args):
        """`script`'s result once truthy; a navigation in between is a retry: no context (a WebDriver error), or, on
        WebKitWebDriver, the request's connection closed when the page it ran in went away (B6-3a's sign-in page submits
        itself as the window opens). A driver that is really gone fails every retry and the poll times out naming `what`."""
        def probe():
            try:
                return self.js(script, *args)
            except (Fail, OSError):
                return None
        return until(what, seconds, probe, every=0.5)

    def url(self):
        return self.poll("the window's URL", 30, "return location.href;")

    def click(self, css, leaves=False):
        """`leaves`: as js's."""
        found = self.call("POST", f"/session/{self.sid}/element", {"using": "css selector", "value": css})
        self._leaving(leaves, lambda: self.call("POST", f"/session/{self.sid}/element/{next(iter(found.values()))}/click", {}))

    def close(self):
        try:
            self.call("DELETE", f"/session/{self.sid}", timeout=20)
        except (Fail, OSError):
            pass
        if WIN:
            run(["taskkill", "/F", "/T", "/PID", str(self.proc.pid)])
        else:
            if LINUX:
                run(["pkill", "-P", str(self.proc.pid), "-x", "WebKitWebDriver"])  # tauri-driver's own, nobody else's
            self.proc.terminate()
        try:
            self.proc.wait(10)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def app_page_shown(win, start):
    """The window shows the engine's page: its URL is this engine's origin and the desktop mark is set."""
    got = win.poll("the window on the engine's page", 60,
                   "return location.origin === arguments[0] && document.readyState === 'complete' && "
                   "document.documentElement.classList.contains('tauri') && location.href;", f"http://127.0.0.1:{start.port}")
    say(f"the window shows {got}")
    return got


def window_text(win, what, contains, seconds=60):
    return win.poll(what, seconds, "var t = (document.body ? document.body.innerText : '').replace(/\\s+/g, ' ').trim(); "
                    "return t.indexOf(arguments[0]) >= 0 && t;", " ".join(contains.split()))


def top_window(pid):
    """Windows: the app's top-level window titled Deltabadger -> (hwnd, visible)."""
    import ctypes
    from ctypes import wintypes
    user32 = ctypes.windll.user32
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def each(hwnd, _):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid:
            title = ctypes.create_unicode_buffer(256)
            user32.GetWindowTextW(hwnd, title, 256)
            if title.value == "Deltabadger":
                found.append(hwnd)
        return True
    user32.EnumWindows(each, 0)
    return found


def window_visible(pid):
    if WIN:
        import ctypes
        return any(ctypes.windll.user32.IsWindowVisible(h) for h in top_window(pid))
    return run(["xdotool", "search", "--onlyvisible", "--name", "^Deltabadger$"]).stdout.split() != []


def close_window(pid):
    """The window's close button: WM_CLOSE on Windows, WM_DELETE_WINDOW on X11 (what a window manager sends)."""
    import ctypes
    if WIN:
        for h in top_window(pid):
            ctypes.windll.user32.PostMessageW(h, 0x0010, 0, 0)
        return
    ids = [int(w) for w in run(["xdotool", "search", "--onlyvisible", "--name", "^Deltabadger$"]).stdout.split()]
    check(ids, "no visible Deltabadger window to close")
    x = ctypes.CDLL("libX11.so.6")
    x.XOpenDisplay.restype = ctypes.c_void_p
    x.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x.XInternAtom.restype = ctypes.c_ulong
    x.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.c_void_p]
    x.XFlush.argtypes = x.XCloseDisplay.argtypes = [ctypes.c_void_p]

    class ClientMessage(ctypes.Structure):  # XClientMessageEvent, padded to sizeof(XEvent)
        _fields_ = [("type", ctypes.c_int), ("serial", ctypes.c_ulong), ("send_event", ctypes.c_int),
                    ("display", ctypes.c_void_p), ("window", ctypes.c_ulong), ("message_type", ctypes.c_ulong),
                    ("format", ctypes.c_int), ("l", ctypes.c_long * 5), ("pad", ctypes.c_long * 24)]
    d = x.XOpenDisplay(None)
    check(d, "no X display")
    for w in ids:
        ev = ClientMessage(type=33, window=w, message_type=x.XInternAtom(d, b"WM_PROTOCOLS", 0), format=32)
        ev.l[0] = x.XInternAtom(d, b"WM_DELETE_WINDOW", 0)
        x.XSendEvent(d, w, 0, 0, ctypes.byref(ev))
    x.XFlush(d)
    x.XCloseDisplay(d)


# ---------- what an install leaves alone ----------

def install_tree():
    """The installed app's files, to show the app writes nothing there. AppImage: compared later with its own contents."""
    if ARGS.kind == "nsis":
        return tree(ARGS.app.parent)
    if ARGS.kind == "deb":
        return {**tree("/usr/lib/Deltabadger"), **{p: tree(Path(p).parent).get(Path(p).name) for p in ("/usr/bin/Deltabadger", "/usr/bin/deltabadger-engine")}}
    if ARGS.kind == "macos":
        return tree(ARGS.app)
    return None


def check_install_unchanged(before, app_pid):
    if ARGS.kind == "appimage":
        # The running app's extracted tree against the AppImage's own contents.
        root = Path(os.readlink(f"/proc/{app_pid}/exe")).parents[2]
        ref = ARGS.work / "appimage-contents"
        if not ref.exists():
            ref.mkdir()
            env = {k: v for k, v in os.environ.items() if k not in ("APPIMAGE_EXTRACT_AND_RUN", "NO_CLEANUP")}
            run([str(ARGS.app), "--appimage-extract"], cwd=ref, env=env)
        want, got = tree(ref / "squashfs-root"), tree(root)
        check(want, "the AppImage's contents could not be listed")
    else:
        want, got = before, install_tree()
    changed = sorted(k for k in set(want) | set(got) if want.get(k) != got.get(k))
    if ARGS.kind == "nsis":
        # B7-0's update lock (rust/src/bundle.rs): per account in %LOCALAPPDATA%\Deltabadger, which is the NSIS install folder.
        changed = [k for k in changed if not (k.startswith("bundle-") and k.endswith(".lock") and "/" not in k and "\\" not in k)]
    check(not changed, f"files written in the install: {changed[:20]}")
    say(f"nothing written in the install ({len(got)} files compared)")


# ---------- scenarios ----------

def fresh_dir(name):
    d = ARGS.work / name / "data"
    check(ARGS.work in d.parents, f"{d} is outside the work dir")
    if d.parent.exists():
        shutil.rmtree(d.parent)
    d.mkdir(parents=True)
    return d


def secrets_same(data, before, when):
    now = stamp(data / ".secrets")
    check(now == before, f".secrets changed {when}: {before} -> {now}")
    say(f"P8 .secrets unchanged {when}: {now[0]} mtime {now[1]}")


def p1(data, start, before_tree, win=None):
    if win:
        app_page_shown(win, start)
    check(http(start.port, "/health-check") == 200, f"/health-check: {http(start.port, '/health-check')}")
    engines = engine_pids()
    check(engines == [start.pid], f"engines: {engines}, the launcher started {start.pid}")
    addrs = listening(start.pid)
    check(addrs and all(a.startswith("127.0.0.1:") for a in addrs), f"the engine listens on {addrs}")
    check(lock_state(data) == "held", f"lock {lock_state(data)}")
    check((data / "db" / "production.sqlite3").exists(), "no primary under the data dir")
    say(f"P1: /health-check 200; one engine ({start.pid}) listening on {addrs} only; lock held; data in {data / 'db'}")
    check_install_unchanged(before_tree, the_one("app", app_pids()))


def scenario_fresh(name):
    """P1 fresh start and (Windows, Linux) P2 a second launch; P8 across them."""
    data = fresh_dir(name)
    before_tree = install_tree()
    if MAC:
        launch(data)
        check_data_line(data)
        start = wait_start(data, 0)
        p1(data, start, before_tree)
        return
    win = Window(data)
    try:
        check_data_line(data)
        start = wait_start(data, 0)
        p1(data, start, before_tree, win)
        secrets = stamp(data / ".secrets")
        app = the_one("app", app_pids())
        close_window(app)
        until("the window hidden by its close button", 10, lambda: not window_visible(app))
        say("P2: the window is hidden (close button); the app keeps running")
        t0 = time.monotonic()
        second = launch(data)
        code = second.wait(10)
        check(code == 0, f"P2: the second launch exited {code}")
        say(f"P2: the second launch exited 0 in {time.monotonic() - t0:.1f} s")
        until("P2: the first window visible again", 10, lambda: window_visible(app))
        check(app_pids() == [app], f"P2: apps {app_pids()}")
        check(engine_pids() == [start.pid], f"P2: engines {engine_pids()}")
        check(starts(data) == 1, f"P2: {starts(data)} engine starts")
        check("desktop: the app was opened again" in lines(data), "P2: no hand-over line")
        say("P2: one app, one engine, one start; the first window is visible again")
        stop_hard_and_drain(data, start, "end")
        secrets_same(data, secrets, "after P2")
    finally:
        win.close()


def scenario_quit(name):
    """P3: macOS `osascript quit`; Linux `kill -TERM` of the app. The engine stops on request, exit 0, lock free."""
    data = fresh_dir(name)
    launch(data)
    check_data_line(data)
    start = wait_start(data, 0)
    secrets = stamp(data / ".secrets")
    if MAC:
        mac_quit(data, start, "P3 osascript quit")
    else:
        watch = ExitWatch(start.pid)
        kill(the_one("app", app_pids()), hard=False)
        until("the app gone", 15, lambda: not app_pids())
        code = watch.wait(60)
        check(code == 0, f"P3: engine exit {code}")
        check(logged(data, "stopped on request"), "P3: no 'stopped on request'")
        check(lock_state(data) == "free", f"P3: lock {lock_state(data)}")
        say(f"P3 kill -TERM of the app: the engine stopped on request, exit {code}, lock free")
    secrets_same(data, secrets, "after P3")


def scenario_app_killed(name):
    """P4: the app force-killed -> the engine exits 0 on its own, lock free; relaunch serves."""
    data = fresh_dir(name)
    launch(data)
    check_data_line(data)
    start = wait_start(data, 0)
    secrets = stamp(data / ".secrets")
    stop_hard_and_drain(data, start, "P4 " + ("taskkill /F" if WIN else "kill -9"))
    secrets_same(data, secrets, "after P4")
    launch(data)
    start = wait_start(data, 1)
    check(http(start.port, "/health-check") == 200, "P4: relaunch /health-check")
    say("P4: relaunch serves")
    stop_hard_and_drain(data, start, "end")
    secrets_same(data, secrets, "after the relaunch")


def scenario_engine_killed(name):
    """P5: the engine killed while the app runs -> D4 in the window; relaunch serves."""
    data = fresh_dir(name)
    win = Window(data)
    try:
        check_data_line(data)
        start = wait_start(data, 0)
        app_page_shown(win, start)
        secrets = stamp(data / ".secrets")
        kill(start.pid)
        until("the launcher showing D4", 20, lambda: logged(data, "desktop: showing stopped"))
        lead = win.poll("D4 in the window", 30, "var e = document.getElementById('lead'); "
                        "return e && e.textContent.indexOf('Deltabadger stopped') === 0 && e.textContent;")
        check(lead == "Deltabadger stopped unexpectedly.", f"P5: the window says {lead!r}")
        exit_block = [x for x in since_start(data) if x.startswith("--- the engine stopped (exit")]
        check(exit_block, "P5: no exit block in the log")
        say(f"P5: engine killed -> the window shows {lead!r}; log: {exit_block[0]}")
        for pid in app_pids():
            kill(pid)
        until("the app gone", 15, lambda: not app_pids())
    finally:
        win.close()
    secrets_same(data, secrets, "after P5")
    launch(data)
    start = wait_start(data, 1)
    check(http(start.port, "/health-check") == 200, "P5: relaunch /health-check")
    say("P5: relaunch serves")
    stop_hard_and_drain(data, start, "end")
    secrets_same(data, secrets, "after the relaunch")


def base_install(name):
    """An install made by the app itself, stopped: what the refusals start from."""
    data = fresh_dir(name)
    launch(data)
    check_data_line(data)
    start = wait_start(data, 0)
    if MAC:
        mac_quit(data, start, "base install")
    else:
        stop_hard_and_drain(data, start, "base install")
    return data


def copy_of(base, name):
    data = fresh_dir(name)
    shutil.rmtree(data)
    shutil.copytree(base, data, copy_function=shutil.copy2, ignore=shutil.ignore_patterns("log"))
    return data


def refusal(data, what, want, **extra):
    before = stamp(data / ".secrets") if (data / ".secrets").exists() else None
    win = Window(data, **extra)
    try:
        check_data_line(data)
        start = wait_start(data, 0, page="refused")
        text = window_text(win, f"{what}: the refusal page", want)
        say(f"{what}: the window shows: {text}")
        check("refused: showing why on port" in "\n".join(since_start(data)), f"{what}: no refusal line in the log")
        stop_hard_and_drain(data, start, what, refused=True)
    finally:
        win.close()
    if before is None:
        say(f"P8 {what}: .secrets minted by the launcher: {stamp(data / '.secrets')[0]}")
    else:
        secrets_same(data, before, f"across {what}")


def scenario_refusals(name):
    """C9-D (the key pair replaced), Unrecognised (a foreign SQLite file at the primary path), DATABASE_URL."""
    base = base_install(f"{name}-base")
    d = copy_of(base, f"{name}-c9")
    s = (d / ".secrets").read_text()
    s2 = re.sub(r"(ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY=)[0-9a-f]+", lambda m: m.group(1) + "0" * 64, s)
    check(s2 != s, "no primary key line to replace")
    (d / ".secrets").write_text(s2)
    refusal(d, "C9-D", f"Deltabadger didn't start: it can't read your data with the keys in {d / '.secrets'}. If that file "
            "was replaced or deleted, quit Deltabadger, put the original file back, and open it again. If you set "
            "Deltabadger's keys yourself, they must be the ones this data was made with. Nothing was changed.")

    d = fresh_dir(f"{name}-foreign")
    (d / "db").mkdir()
    c = sqlite3.connect(d / "db" / "production.sqlite3")
    c.execute("create table notes(x)")
    c.execute("insert into notes values (1)")
    c.commit()
    c.close()
    refusal(d, "Unrecognised", f"Deltabadger didn't start: the data in {d / 'db'} isn't data this version can open. "
            f"Nothing was changed. Contact support and send them the files in {d / 'log'}.")

    d = copy_of(base, f"{name}-dburl")
    refusal(d, "DATABASE_URL", "Deltabadger didn't start: a DATABASE_URL setting on this computer points it elsewhere. "
            "Remove it, then open Deltabadger again. Nothing was changed.", DATABASE_URL="postgres://localhost/elsewhere")


def scenario_takeover(name):
    """P7: the 2.x desktop dir (fixture_2x.sh) taken over: 'took over', a complete pre-3.0 backup, the app served."""
    check(ARGS.fixture_2x, "--fixture-2x is required")
    data = fresh_dir(name)
    with tarfile.open(ARGS.fixture_2x) as t:
        t.extractall(data, filter="tar")  # modes and mtimes as the fixture job made them
    check((data / ".secrets").exists() and (data / "db" / "production.sqlite3").exists(), "the 2.x fixture is incomplete")
    secrets = stamp(data / ".secrets")
    launch(data)
    check_data_line(data)
    start = wait_start(data, 0, seconds=600)
    took = [x for x in lines(data) if "took over" in x]
    check(took, "P7: no 'took over' in the log")
    backups = sorted(p for p in (data / "db" / "backups").iterdir() if p.name.startswith("pre-3.0-"))
    check(len(backups) == 1 and not backups[0].name.endswith(".partial"), f"P7: backups {[b.name for b in backups]}")
    digest = (backups[0] / "digest").read_text().split("\n")
    entries = [x.split("  ", 1) for x in digest if x]
    check(len(entries) == 2, f"P7: digest lists {entries}")
    for sha, file in entries:
        got = hashlib.sha256((backups[0] / file).read_bytes()).hexdigest()
        check(got == sha, f"P7: {file} in the backup does not match its digest")
    check(http(start.port, "/health-check") == 200, "P7: /health-check")
    primaries = [p for p in data.rglob("production.sqlite3") if "backups" not in p.parts]
    check(len(primaries) == 1, f"P7: primaries {primaries}")
    say(f"P7: {took[0]} | backup {backups[0].name} ({', '.join(f for _, f in entries)}; digest matches) | /health-check 200 | one primary")
    if MAC:
        mac_quit(data, start, "end")
    else:
        stop_hard_and_drain(data, start, "end")
    secrets_same(data, secrets, "across the takeover")


def downloads_dir():
    if LINUX:
        return Path(os.environ["SMOKE_DOWNLOADS"])
    return Path.home() / "Downloads"


def new_file(folder, before, prefix, seconds=30):
    def probe():
        got = [p for p in folder.glob(prefix + "*") if p.name not in before and not p.name.endswith((".crdownload", ".tmp", ".partial"))]
        return got and got[0].stat().st_size > 0 and got
    return until(f"a new {prefix}* in {folder}", seconds, probe)[0]


SIGNED_IN = """
var path = location.pathname;
if (document.readyState !== 'complete' || document.querySelector('form input[type=password], [data-desktop-sign-in]')) { return null; }
if (/^\\/([a-z]{2}(-[A-Za-z]{2})?\\/)?(login|signup|setup|desktop)(\\/|$)/.test(path)) { return null; }
return location.href;
"""


def scenario_downloads(name):
    """The tracker export (a CSV filtered by the chosen dates) and the API docs, saved to the Downloads folder."""
    data = fresh_dir(name)
    folder = downloads_dir()
    folder.mkdir(parents=True, exist_ok=True)
    win = Window(data)
    try:
        check_data_line(data)
        start = wait_start(data, 0)
        app_page_shown(win, start)
        # B6-3a: no sign-up; the window opens signed in as the one desktop account.
        win.poll("signed in", 60, SIGNED_IN)
        say(f"opened signed in; the window is at {win.url()}")
        # Three transactions; the chosen window holds the middle one only.
        c = sqlite3.connect(data / "db" / "production.sqlite3", timeout=30)
        users = c.execute("SELECT id, email FROM users").fetchall()
        check(len(users) == 1 and users[0][1] == "desktop@deltabadger.invalid", f"users: {len(users)}")
        user = users[0][0]
        exchange = c.execute("SELECT id FROM exchanges ORDER BY id LIMIT 1").fetchone()[0]
        for tx, at in (("smoke-early", "2024-01-15 12:00:00"), ("smoke-mid", "2024-06-15 12:00:00"), ("smoke-late", "2024-12-15 12:00:00")):
            c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, "
                      "quote_amount, transacted_at, created_at, updated_at, tx_id) VALUES (?, ?, 0, 'BTC', '0.1', 'USD', '100', ?, ?, ?, ?)",
                      (user, exchange, at, at, at, tx))
        c.commit()
        c.close()
        page = win.url()
        before = {p.name for p in folder.iterdir()}
        # What tracker_export_controller.js does on Download with a date range: a navigation to the export URL.
        win.js("location.href = '/tracker/export?from=2024-03-01&to=2024-09-30'; return true;", leaves=True)
        csv = new_file(folder, before, "deltabadger-transactions-")
        rows = csv.read_text(encoding="utf-8").splitlines()
        check(rows[0].startswith("date,type,base_currency,base_amount"), f"export header: {rows[:1]}")
        check(len(rows) == 2 and "smoke-mid" in rows[1] and rows[1].startswith("2024-06-15T12:00:00Z"), f"export rows: {rows}")
        time.sleep(1)
        check(win.url() == page, f"the export navigated the window away: {win.url()}")
        say(f"tracker export saved as {csv.name}: {rows[1]}; the window stayed at {page}")
        before = {p.name for p in folder.iterdir()}
        # A click, as on the Settings button: WebView2 blocks a second download a page starts without a user gesture.
        win.js("var a = document.createElement('a'); a.id = 'smoke-docs'; a.dataset.turbo = 'false';"
               " a.href = '/settings/download_api_docs'; a.textContent = 'deltabadger-api.md';"
               " a.style.cssText = 'position:fixed;left:40%;top:40%;padding:1em;z-index:2147483647;background:#fff'; document.body.append(a); return true;")
        win.click("#smoke-docs", leaves=True)
        docs = new_file(folder, before, "deltabadger-api")
        check(docs.read_bytes() == (REPO / "manual" / "api.md").read_bytes(), f"{docs.name} differs from manual/api.md")
        say(f"API docs saved as {docs.name}: byte-equal to manual/api.md ({docs.stat().st_size} bytes)")
        saved = [x for x in since_start(data) if x == "desktop: download saved"]
        check(len(saved) == 2, f"download lines: {saved}")
        stop_hard_and_drain(data, start, "end")
    finally:
        win.close()


SCENARIOS = {
    "fresh": scenario_fresh,
    "quit": scenario_quit,
    "app-killed": scenario_app_killed,
    "engine-killed": scenario_engine_killed,
    "refusals": scenario_refusals,
    "takeover": scenario_takeover,
    "downloads": scenario_downloads,
}


def production_dir():
    if WIN:
        return Path(os.environ["APPDATA"]) / "Deltabadger"
    if MAC:
        return Path.home() / "Library" / "Application Support" / "com.deltabadger.app"
    return Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local" / "share") / "com.deltabadger.app"


def print_logs(name):
    for data in sorted((ARGS.work).glob(f"{name}*/data")):
        for f in ("deltabadger.log", "deltabadger.log.1", "deltabadger-stderr.log", "deltabadger-stderr.log.1"):
            p = data / "log" / f
            if p.exists():
                print(f"::group::{p}")
                print(p.read_text(encoding="utf-8", errors="replace")[-20000:])
                print("::endgroup::")


def main():
    global ARGS
    ap = argparse.ArgumentParser()
    ap.add_argument("--kind", required=True, choices=["nsis", "appimage", "deb", "macos"])
    ap.add_argument("--app", required=True, type=lambda p: Path(p).resolve())
    ap.add_argument("--work", required=True, type=lambda p: Path(p).resolve())
    ap.add_argument("--fixture-2x", type=lambda p: Path(p).resolve())
    ap.add_argument("scenarios", nargs="+", choices=list(SCENARIOS))
    ARGS = ap.parse_args()
    check(ARGS.app.exists(), f"no app at {ARGS.app}")
    # On macOS the app's processes are found by the test .app's own path; elsewhere by process name, which an installed
    # Deltabadger shares, so those kinds run only on a CI runner, where nothing else is (cleanup kills what it finds).
    check(MAC or os.environ.get("CI") == "true", f"--kind {ARGS.kind} runs on a CI runner only (CI=true)")
    if MAC:
        # The released identifier ignores DELTABADGER_DESKTOP_DATA_DIR and would open the real data: never launched.
        import plistlib
        with open(ARGS.app / "Contents" / "Info.plist", "rb") as f:
            got = plistlib.load(f).get("CFBundleIdentifier")
        check(got == TEST_ID, f"{ARGS.app} is {got}, not the test build {TEST_ID}")
    ARGS.work.mkdir(parents=True, exist_ok=True)
    check(not app_pids() and not engine_pids(), f"the app is already running: {app_pids()} {engine_pids()}")
    if LINUX:
        import ctypes
        check(ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0, "PR_SET_CHILD_SUBREAPER")  # read the orphaned engine's exit

    # r2.2: the released app's data dir, seeded with sentinels on a clean CI runner only, is never opened by a test build.
    sentinels = None
    prod = production_dir()
    if os.environ.get("CI") == "true" and not prod.exists():
        (prod / "db").mkdir(parents=True)
        for f in (".secrets", "db/production.sqlite3", "db/production_queue.sqlite3"):
            (prod / f).write_bytes(b"sentinel: a test build must never open this\n")
        sentinels = tree(prod), {f: (prod / f).stat().st_mtime_ns for f in tree(prod)}

    results = []
    for name in ARGS.scenarios:
        say(f"=== {name} ===")
        t0 = time.monotonic()
        try:
            SCENARIOS[name](name)
            outcome = "passed"
        except Exception as e:  # every scenario runs; the run fails at the end
            outcome = f"FAILED: {e}"
            traceback.print_exc()
            print_logs(name)
        finally:
            cleanup()
        results.append((name, outcome, time.monotonic() - t0))
        say(f"=== {name}: {outcome} ({results[-1][2]:.0f} s) ===")

    if sentinels is not None:
        now = tree(prod), {f: (prod / f).stat().st_mtime_ns for f in tree(prod)}
        outcome = "passed" if now == sentinels else f"FAILED: the production dir changed: {sorted(now[0])}"
        results.append(("production dir untouched", outcome, 0))
    summary = [f"| {ARGS.kind} | {n} | {o} | {s:.0f} s |" for n, o, s in results]
    print("\n".join(summary))
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as f:
            f.write("| leg | scenario | outcome | time |\n|---|---|---|---|\n" + "\n".join(summary) + "\n")
    sys.exit(0 if all(o == "passed" for _, o, _ in results) else 1)


if __name__ == "__main__":
    main()
