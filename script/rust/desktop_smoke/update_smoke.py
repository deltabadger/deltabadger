#!/usr/bin/env python3
"""The desktop app's update proofs on Windows and Linux (PLAN-B7 §9 B7-2, r1.9, r1.10, r2.3, r3.3, r3.7, r4.2, r4.3,
r5.1, r5.2, r5.4), run by desktop.yml after smoke.py, against the installed test build:

    update_smoke.py --kind nsis|appimage|deb --packages <update_builds.sh's dir> --work <empty dir> <scenario>...

N is the job's own build, N+1 the same tree with its version bumped and one proof migration (update_builds.sh), signed
with the job's throwaway key; the feed is served here, on the endpoint the test build was built with
(http://127.0.0.1:8765/latest.json), assembled by script/release/validate.sh `feed`, and every request is logged.
Windows runs the NSIS install in the runner's own account, on the test identifier's own data dir and without the data
dir override (r1.9: the installer's relaunch doesn't carry the environment). Linux runs the AppImage mounted (FUSE) on
DELTABADGER_DESKTOP_DATA_DIR. The tray's Quit is the test identifier's trigger file (`<data>/test-user-quit`, the same
`user_quit`); the window is read over WebDriver (smoke.py `Window`). Python 3 standard library only.
"""
import argparse
import hashlib
import http.server
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import threading
import time
import traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import smoke as s  # noqa: E402
from smoke import Fail, check, say, until  # noqa: E402

PORT = 8765
WIN, LINUX = s.WIN, s.LINUX
REPO = s.REPO
PACKAGES = {"windows-x86_64": "Deltabadger_windows-x86_64-setup.exe", "linux-x86_64-appimage": "Deltabadger_linux-x86_64.AppImage",
            "darwin-aarch64": "Deltabadger_darwin-aarch64.app.tar.gz"}
TRAY_LINE = ("Deltabadger finishes its current work before it restarts. You can also update later: choose Quit from the "
             "Deltabadger icon's menu, and it installs the update before it closes.")
BEING_UPDATED = "this copy of Deltabadger is being updated; open it again in a moment"


# ---------- the feed ----------

class Feed:
    dir = None
    throttle = 0  # bytes per second; 0 = as fast as it goes
    requests = []  # (path, Range header)


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        name = self.path.split("?")[0].lstrip("/")
        Feed.requests.append((name, self.headers.get("Range")))
        path = Feed.dir / name if Feed.dir and name and "/" not in name and ".." not in name else None
        if not path or not path.is_file():
            self.send_error(404)
            return
        data = path.read_bytes()
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        step = 64 * 1024
        for i in range(0, len(data), step):
            try:
                self.wfile.write(data[i:i + step])
            except OSError:
                return
            if Feed.throttle:
                time.sleep(step / Feed.throttle)

    def log_message(self, *args):
        pass


def package_name():
    return PACKAGES["windows-x86_64" if WIN else "linux-x86_64-appimage"]


def package_gets():
    return [r for r in Feed.requests if r[0] == package_name()]


def publish(variant="next", critical=False):
    """A feed of N+1 (or a variant of it, P3/r5.1) at the endpoint, assembled by validate.sh `feed`."""
    d = ARGS.work / "feeds" / f"{variant}-{time.monotonic_ns()}"
    d.mkdir(parents=True)
    src, sig = P / V["package"], P / (V["package"] + ".sig")
    if variant == "foreign":
        src, sig = P / "foreign.bin", P / "foreign.bin.sig"
    if variant == "other-key":
        sig = P / (V["package"] + ".other.sig")
    if variant == "bad-bytes":
        data = bytearray(src.read_bytes())
        data[-1] ^= 1
        (d / package_name()).write_bytes(bytes(data))
    else:
        shutil.copyfile(src, d / package_name())
    shutil.copyfile(sig, d / (package_name() + ".sig"))
    for other in PACKAGES.values():
        if other != package_name():
            (d / (other + ".sig")).write_text("not this platform's\n")
    bash = r"C:\Program Files\Git\bin\bash.exe" if WIN else "bash"
    out = s.run([bash, (REPO / "script/release/validate.sh").as_posix(), "feed", d.as_posix(), f"v{V['next']}",
                 "true" if critical else "false", f"http://127.0.0.1:{PORT}"])
    check(out.returncode == 0, f"validate.sh feed: {out.stderr}")
    if variant == "version":
        feed = json.loads((d / "latest.json").read_text())
        major, minor, patch = V["next"].split(".")
        feed["version"] = f"{major}.{minor}.{int(patch) + 1}"  # not the version the package was signed for
        (d / "latest.json").write_text(json.dumps(feed))
    Feed.dir = d
    say(f"feed: {variant}{' (critical)' if critical else ''}")


# ---------- the installed app ----------

def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def data_dir(name):
    if WIN:
        return Path(os.environ["APPDATA"]) / s.TEST_ID
    return ARGS.work / name / "data"


def env_for(data, **extra):
    """smoke.py's launch environment: no data dir override on Windows (r1.9)."""
    env = dict(os.environ, **extra)
    if WIN:
        env.pop("DELTABADGER_DESKTOP_DATA_DIR", None)
    else:
        env["DELTABADGER_DESKTOP_DATA_DIR"] = str(data)
        env.pop("APPIMAGE_EXTRACT_AND_RUN", None)
    return env


INST = Path(os.environ.get("LOCALAPPDATA", "/nonexistent")) / "Deltabadger"
HOOKS = Path(os.environ.get("LOCALAPPDATA", "/nonexistent")) / "deltabadger-test-hooks"


def install_n(name, debug=False):
    """A fresh N and a fresh data dir. Windows: N's installer, silent, into the runner's account; an AppImage: a copy of N
    in its own folder."""
    data = data_dir(name)
    if data.exists():
        shutil.rmtree(data)
    data.mkdir(parents=True)
    if WIN:
        shutil.rmtree(HOOKS, ignore_errors=True)
        HOOKS.mkdir(parents=True)
        p = subprocess.run([str(P / "N-setup.exe"), "/S"])
        rm = (HOOKS / "rm-result").read_text(errors="replace").strip() if (HOOKS / "rm-result").exists() else "none"
        check(p.returncode == 0, f"N's installer exited {p.returncode}; Restart Manager: {rm}; engines {s.engine_pids()}")
        check(not (INST / ".updating").exists(), "a fresh install left .updating")
        if debug:
            shutil.copyfile(P / "engine-debug.exe", INST / "deltabadger-engine.exe")
        s.ARGS.app = INST / "Deltabadger.exe"
    elif ARGS.kind == "appimage":
        folder = ARGS.work / name / "app"
        if folder.exists():
            folder.chmod(0o755)
            shutil.rmtree(folder)
        folder.mkdir(parents=True)
        s.ARGS.app = folder / "Deltabadger.AppImage"
        shutil.copyfile(P / ("N-debug.AppImage" if debug else "N.AppImage"), s.ARGS.app)
        s.ARGS.app.chmod(0o755)
    return data


def installed():
    """What decides which version runs: Windows both executables; an AppImage the file at $APPIMAGE."""
    if WIN:
        return sha(INST / "Deltabadger.exe"), sha(INST / "deltabadger-engine.exe")
    return (sha(s.ARGS.app),)


def next_pair():
    return (sha(P / "next-Deltabadger.exe"), sha(P / "next-deltabadger-engine.exe"))


def is_next():
    if WIN:
        return installed() == next_pair()
    return installed() == (sha(P / V["package"]),)


def install_files():
    return s.tree(INST) if WIN else {"app": sha(s.ARGS.app)}


def window(data, **extra):
    return s.Window(data, **extra)


def launch(data, **extra):
    return s.launch(data, **extra)


def quit_from_tray(data):
    (data / "test-user-quit").write_text("")


def state_lines(data, *what):
    """The agent's state lines (lib.rs: `state key=value…`, in no fixed order) carrying every one of `what`."""
    return [x for x in s.lines(data) if x.startswith("desktop: update: state ") and all(w in x for w in what)]


def wait_state(data, *what, seconds=180, more_than=0):
    until(f"update state {what}", seconds, lambda: len(state_lines(data, *what)) > more_than)


def version_starts(data, version):
    return [i for i, x in enumerate(s.lines(data)) if x == f"desktop: version {version}"]


def check_runs(data, version, what, before=0):
    """A start of this version after the `before` ones on `data`: its data: line before its engine (r1.9), the engine's
    page, /health-check."""
    until(f"{what}: version {version} started", 120, lambda: len(version_starts(data, version)) > before)
    def served():
        got = s.lines(data)[version_starts(data, version)[-1]:]
        return any(x == "desktop: showing the engine's app page" for x in got) and got
    got = until(f"{what}: {version}'s app page", 300, served)
    data_at = next(i for i, x in enumerate(got) if x.startswith("data: "))
    started = [i for i, x in enumerate(got) if x.startswith("desktop: the engine started on port ")]
    check(started and data_at < started[0], f"{what}: the data line is not before the engine's start")
    port = int(got[started[-1]].split(" port ")[1].split(" ")[0])
    check(s.http(port, "/health-check") == 200, f"{what}: /health-check")
    say(f"{what}: {version} serves on the same data (port {port}); its data line came before its engine")
    return port


def check_next_migrated(data, what):
    c = sqlite3.connect(data / "db" / "production.sqlite3", timeout=30)
    tables = {r[0] for r in c.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    c.close()
    check("b7_proof" in tables, f"{what}: N+1's proof migration is not applied")
    copies = [p.name for p in (data / "db" / "backups").iterdir() if p.name.startswith("before-update-")]
    check(len(copies) == 1, f"{what}: before-update copies {copies}")
    say(f"{what}: N+1's proof migration applied; one {copies[0]}")


def check_n_unchanged(before, what):
    now = install_files()
    changed = sorted(k for k in set(before) | set(now) if before.get(k) != now.get(k) and not (k.startswith("bundle-") and k.endswith(".lock")))
    check(not changed, f"{what}: N's files changed: {changed[:10]}")
    say(f"{what}: N's files byte-equal ({len(now)} compared)")


def engine_port(data):
    got = s.since_start(data)
    started = [x for x in got if x.startswith("desktop: the engine started on port ")]
    return int(started[-1].split(" port ")[1].split(" ")[0]) if started else None


def check_n_serves(data, what):
    port = until(f"{what}: N's engine", 120, lambda: "desktop: showing the engine's app page" in s.since_start(data) and engine_port(data))
    check(s.http(port, "/health-check") == 200, f"{what}: N's /health-check")
    say(f"{what}: N serves (port {port})")


BANNER = """
var b = document.getElementById('desktop_update');
if (!b || b.classList.contains('hidden')) { return null; }
var q = function (sel) { var e = b.querySelector(sel); return e ? e.textContent : ''; };
var r = {line: q('[data-desktop-update-line]'), more: q('[data-desktop-update-more]'), danger: b.classList.contains('salert--danger')};
return r.line.indexOf(arguments[0]) === 0 ? r : null;
"""


def banner(win, line, seconds=180):
    got = win.poll(f"the banner saying {line!r}", seconds, BANNER, line)
    say(f"banner: {got['line']!r} / {got['more']!r}{' (red)' if got['danger'] else ''}")
    return got


def install_click(win, data):
    """The banner's update button. WebKitWebDriver may drop a click's connection when the page navigates, and a click
    sent while the page loads (the start's exchange modal) is lost the same way: the app's own line says if it landed.
    A second click after one that landed is refused by the app ("an install or a quit is under way"), never a 2nd install."""
    asked = "desktop: update: install asked for"
    for _ in range(3):
        n = s.lines(data).count(asked)
        try:
            win.click("[data-desktop-update-action]", leaves=True)
            return until("the click's install request", 10, lambda: s.lines(data).count(asked) > n)
        except (Fail, OSError):
            say("the click did not land (the page was busy); again")
    raise Fail("the update button: 3 clicks, no install asked for")


def status_text(win, what, lead, seconds=60):
    got = win.poll(what, seconds, "var e = document.getElementById('lead'); return e && e.textContent === arguments[0] && e.textContent;", lead)
    say(f"{what}: the window says {got!r}")


class Seen:
    """When `probe` first turned true, polled on a thread."""

    def __init__(self, probe, every=0.1, then=None):
        self.at, self.value, self.stop, self.then = None, None, False, None

        def loop():
            while not self.stop and self.at is None:
                v = probe()
                if v:
                    # `then` runs on the first sight (an installer's exit watch needs a handle opened while it runs).
                    self.then = then and then(v)
                    self.at, self.value = time.monotonic(), v
                time.sleep(every)
        threading.Thread(target=loop, daemon=True).start()


def command_line(pid):
    """Windows: a process's command line."""
    ps = f"(Get-CimInstance Win32_Process -Filter 'ProcessId={pid}').CommandLine"
    return s.run(["powershell", "-NoProfile", "-Command", ps]).stdout.strip()


def installer_pids():
    return s.pids("setup") if WIN else []


def cli_engine(name, exe, **extra):
    """An engine of the installed app on another data dir (a command in a terminal): it holds the app's shared lock."""
    d = ARGS.work / f"cli-{name}"
    shutil.rmtree(d, ignore_errors=True)
    (d / "db").mkdir(parents=True)
    port = s.free_port()
    env = {k: v for k, v in os.environ.items() if k != "DELTABADGER_DESKTOP_DATA_DIR"}
    env.update(PORT=str(port), BINDING="127.0.0.1", STORAGE_DIR=str(d / "db"), DATABASE_PATH=str(d / "db" / "production.sqlite3"),
               QUEUE_DATABASE_PATH=str(d / "db" / "production_queue.sqlite3"), APP_TMP_DIR=str(d / "tmp"),
               DELTABADGER_STOP_ON_STDIN_EOF="1", **extra)
    out = open(ARGS.work / f"cli-{name}.out", "wb")
    proc = subprocess.Popen([str(exe), "serve"], env=env, stdin=subprocess.PIPE, stdout=out, stderr=subprocess.STDOUT)
    proc.port, proc.out = port, ARGS.work / f"cli-{name}.out"
    return proc


def cli_up(proc, what):
    until(f"{what}: the CLI engine answering", 120, lambda: proc.poll() is None and s.http(proc.port, "/") is not None)
    say(f"{what}: a CLI engine (process {proc.pid}) runs on another data dir")


def cli_of_the_running_app():
    """The installed app's engine for a CLI run: Windows its file; an AppImage the engine in the running app's mount, with
    the variables the AppImage runtime gives its processes."""
    if WIN:
        return INST / "deltabadger-engine.exe", {}
    app = s.the_one("app", s.app_pids())
    env = dict(x.split("=", 1) for x in open(f"/proc/{app}/environ", "rb").read().decode(errors="replace").split("\0") if "=" in x)
    return Path(env["APPDIR"]) / "usr" / "bin" / s.ENGINE, {"APPIMAGE": env["APPIMAGE"], "APPDIR": env["APPDIR"]}


def no_app_within(seconds, what):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        check(not s.app_pids(), f"{what}: the app is running again: {s.app_pids()}")
        time.sleep(0.5)
    say(f"{what}: no Deltabadger process {seconds} s later")


def installer_seen():
    """Windows: the update's installer, watched from its first sight (it may end before the app has handed over)."""
    return Seen(installer_pids, every=0.05, then=lambda v: s.ExitWatch(v[0]))


def wait_installer_exit(what, seen, seconds=180):
    """Windows: the update's installer appeared, then ends; its exit code."""
    until(f"{what}: the installer started", 120, lambda: seen.then)
    pid = seen.value[0]
    code = seen.then.wait(seconds)
    say(f"{what}: the installer (process {pid}) exited {code}")
    return code


# ---------- scenarios ----------

def scenario_p1_p2_p5(name):
    """P1 the 12 h timer (20 s here) finds a feed published after start; P2 a quit mid-download and a kill after it is
    ready each mean a new download from zero, nothing staged on disk; U2 with the tray's line; P5 the tray's Quit with
    the update ready installs it and closes, the next open runs N+1."""
    data = install_n(name)
    Feed.dir, Feed.throttle = None, 0
    win = window(data, DELTABADGER_TEST_UPDATE_INTERVAL_SECS="20")
    try:
        s.check_data_line(data)
        s.wait_start(data, 0)
        if LINUX:
            check(s.logged(data, "desktop: bundle Some(AppImage), APPIMAGE set") or "desktop: bundle Some(AppImage), APPIMAGE set" in s.lines(data),
                  "bundle_type() and APPIMAGE don't agree on an AppImage")
            say("bundle_type() = AppImage and APPIMAGE set agree")
        until("the start check (no feed yet)", 60, lambda: any("the check failed" in x for x in s.lines(data)))
        Feed.throttle = 1_500_000
        publish()
        until("P1: the timer finds the feed", 90, lambda: any(x.endswith(f"{V['next']} is available") for x in s.lines(data)))
        wait_state(data, 'state="downloading"', "percent=25")
        say("P1: found by the timer after start; P2: downloading at 25 %")
        quit_from_tray(data)
        until("P2: the app quit", 60, lambda: not s.app_pids())
    finally:
        win.close()
    s.cleanup()
    check(len(package_gets()) == 1, f"P2: package requests {package_gets()}")
    Feed.throttle = 0
    for how in ("after a quit at 25 %", "after a kill once ready"):
        win = window(data)
        try:
            got = banner(win, "An update is ready.")
            check(got["more"] == TRAY_LINE, f"U2's second line: {got['more']!r}")
            check(package_gets()[-1][1] is None, f"P2: a ranged request {package_gets()[-1]}")
            say(f"P2 {how}: downloaded again from zero ({len(package_gets())} full requests)")
            big = [p for p in data.rglob("*") if p.is_file() and p.stat().st_size > 10_000_000 and "db" not in p.relative_to(data).parts]
            check(not big, f"P2: a package staged on disk: {big}")
            if how.startswith("after a quit"):
                for pid in s.app_pids():
                    s.kill(pid)
                until("the app gone", 30, lambda: not s.app_pids())
                s.cleanup()
            else:
                quit_from_tray(data)
                until("P5: U12", 30, lambda: any(x == "desktop: update: showing quitting-update" for x in s.lines(data)))
                if WIN:
                    pid = until("P5: the installer started", 120, lambda: installer_pids() and installer_pids()[0])
                    line = command_line(pid)
                    code = s.ExitWatch(pid).wait(180)
                    check(code == 0, f"P5: the installer exited {code}")
                    check(line.split()[-2:] == ["/P", "/UPDATE"], f"P5: the installer's command line {line}")
                    say(f"P5: the installer ran without /R: {line}")
                else:
                    until("P5: the app closed", 120, lambda: not s.app_pids())
                no_app_within(10, "P5")
        finally:
            win.close()
    check(len(package_gets()) == 3, f"package requests {package_gets()}")
    check(is_next(), "P5: N+1 is not installed")
    launch(data)
    check_runs(data, V["next"], "P5: the next open")
    check_next_migrated(data, "P5")


def scenario_p4_p6(name):
    """P4 the click: the installer (Windows /P /UPDATE /R) or the rename over $APPIMAGE (mode kept), N+1 relaunched.
    P6 with the debug engine held in a tick: nothing is replaced while it lives, its exit comes before the install."""
    data = install_n(name, debug=True)
    release = ARGS.work / name / "release"
    release.parent.mkdir(parents=True, exist_ok=True)
    release.unlink(missing_ok=True)
    publish()
    mode = None if WIN else s.ARGS.app.stat().st_mode
    win = window(data, DELTABADGER_TEST_PAUSE="in-tick", DELTABADGER_TEST_RELEASE=str(release))
    try:
        start = s.wait_start(data, 0)
        banner(win, "An update is ready.")
        before = installed()
        engine_file = INST / "deltabadger-engine.exe" if WIN else s.ARGS.app
        stamp = (sha(engine_file), engine_file.stat().st_mtime_ns)
        replaced = Seen(installer_pids if WIN else (lambda: installed() != before))
        install_click(win, data)
        status_text(win, "U7", "Deltabadger is finishing its current work before updating and restarting.")
        end = time.monotonic() + 20
        while time.monotonic() < end:
            check(start.pid in s.engine_pids(), "P6: the held engine is gone")
            check(replaced.at is None, f"P6: replaced while the engine runs: {replaced.value}")
            time.sleep(0.5)
        check((sha(engine_file), engine_file.stat().st_mtime_ns) == stamp, "P6: the engine's file was written while it ran")
        say("P6: 20 s held in a tick: the engine runs, nothing replaced, its file untouched")
        release.write_text("")
        until("P6: the engine's exit after the release", 120, lambda: start.pid not in s.engine_pids(), every=0.05)
        exited = time.monotonic()
        until("P4: replaced", 180, lambda: replaced.at)
        line = command_line(replaced.value[0]) if WIN else ""
        check(replaced.at >= exited - 0.2, "P6: the replacement began before the engine's exit")
        lines = s.lines(data)
        stopped = max(i for i, x in enumerate(lines) if "stopped on request" in x)
        installed_at = next(i for i, x in enumerate(lines) if x == "desktop: update: installed; restarting")
        check(stopped < installed_at, "P6: the install line is before the engine's stop")
        say(f"P6: the engine exited, then the {'installer started' if WIN else 'AppImage was replaced'} ({replaced.at - exited:.1f} s later)")
        if WIN:
            check(line.split()[-3:] == ["/P", "/UPDATE", "/R"], f"P4: the installer's command line {line}")
            say(f"P4: {line}")
    finally:
        win.close()
    check_runs(data, V["next"], "P4: relaunched")
    check(is_next(), f"P4: N+1's executables are not the installed ones: {installed()} vs {next_pair() if WIN else sha(P / V['package'])}")
    if not WIN:
        check(s.ARGS.app.stat().st_mode == mode, f"P4: mode {oct(s.ARGS.app.stat().st_mode)}, was {oct(mode)}")
        say("P4: the file at $APPIMAGE replaced, its mode kept")
    else:
        say("P4: deltabadger-engine.exe and Deltabadger.exe are N+1's (sha256)")
    check_next_migrated(data, "P4")


def scenario_p3(name):
    """P3 a changed package, another key's signature, a feed version that isn't the signed one → U5b; r5.1 another
    platform's package correctly signed under this key → U5d at the install. N byte-equal and serving after each."""
    data = install_n(name)
    before = install_files()
    publish("bad-bytes")
    win = window(data, DELTABADGER_TEST_UPDATE_INTERVAL_SECS="15")
    try:
        s.wait_start(data, 0)
        got = banner(win, "The update couldn't be verified")
        check(got["line"] == "The update couldn't be verified, so it wasn't installed. Deltabadger will try again later.", got["line"])
        for variant in ("other-key", "version"):
            n = len(state_lines(data, 'kind="verify"'))
            publish(variant)
            wait_state(data, 'state="downloading"', more_than=len(state_lines(data, 'state="downloading"')))
            wait_state(data, 'kind="verify"', more_than=n)
            say(f"P3 {variant}: U5b")
        check_n_unchanged(before, "P3")
        check(s.http(engine_port(data), "/health-check") == 200, "P3: N's /health-check")
        publish("foreign")
        banner(win, "An update is ready.")
        install_click(win, data)
        got = banner(win, "The update couldn't be installed.")
        check(any("not installed: the package is not a" in x for x in s.lines(data)), "r5.1: no format refusal in the log")
        say(f"r5.1: another platform's signed package refused: {got['line']!r}")
        check_n_serves(data, "r5.1")
        check_n_unchanged(before, "r5.1")
    finally:
        win.close()


def scenario_p7(name):
    """P7 critical: U4 then U3, red; nothing installed until the click."""
    data = install_n(name)
    before = install_files()
    Feed.throttle = 3_000_000
    publish(critical=True)
    win = window(data)
    try:
        s.wait_start(data, 0)
        got = banner(win, "An important update is downloading")
        check(got["danger"], "P7: U4 is not red")
        Feed.throttle = 0
        got = banner(win, "An important update is ready. Please restart soon to install it.")
        check(got["danger"] and got["more"] == TRAY_LINE, f"P7: U3 {got}")
        time.sleep(15)
        check(not installer_pids(), "P7: an installer runs without a click")
        check_n_unchanged(before, "P7: 15 s after U3")
    finally:
        win.close()


def scenario_p8(name):
    """P8 the install can't happen (Windows: the installer can't be launched from its folder; Linux: the AppImage's folder
    is read-only) → U5d, N's engine started again and serving, N byte-equal."""
    data = install_n(name)
    before = install_files()
    publish()
    extra = {}
    if WIN:
        temp = ARGS.work / name / "noexec"
        temp.mkdir(parents=True, exist_ok=True)
        check(s.run(["icacls", str(temp), "/deny", "*S-1-1-0:(OI)(CI)(X)"]).returncode == 0, "icacls")
        extra = {"TEMP": str(temp), "TMP": str(temp)}
    else:
        s.ARGS.app.parent.chmod(0o555)
    try:
        win = window(data, **extra)
        try:
            start = s.wait_start(data, 0)
            banner(win, "An update is ready.")
            install_click(win, data)
            got = banner(win, "The update couldn't be installed.", seconds=240)
            say(f"P8: {got['line']!r}")
            until("P8: N's engine started again", 120, lambda: s.starts(data) >= 2)
            check(start.pid not in s.engine_pids(), "P8: the first engine still runs")
            check_n_serves(data, "P8")
            check_n_unchanged(before, "P8")
            check(not (data / "updates" / "attempt").exists(), "P8: the failed launch left its attempt")
        finally:
            win.close()
    finally:
        if not WIN:
            s.ARGS.app.parent.chmod(0o755)


def scenario_lock(name):
    """r5.2: an engine of this app on another data dir holds the shared lock → the click waits (U7, U8 after 60 s); the
    tray's Quit then closes the app with nothing installed, the other engine untouched."""
    data = install_n(name)
    before = install_files()
    publish()
    win = window(data)
    try:
        s.wait_start(data, 0)
        exe, extra = cli_of_the_running_app()
        cli = cli_engine(name, exe, **extra)
        cli_up(cli, "r5.2")
        banner(win, "An update is ready.")
        install_click(win, data)
        until("r5.2: U8", 120, lambda: "desktop: update: showing updating-long" in s.lines(data))
        status_text(win, "U8", "Deltabadger is finishing its current work before updating and restarting.")
        win.poll("U8's line", 30, "var e = document.getElementById('more'); return e && e.textContent.indexOf('The update is waiting') === 0;")
        quit_from_tray(data)
        until("r5.2: the app closed", 60, lambda: not s.app_pids())
        check(cli.poll() is None, "r5.2: the other engine stopped")
        check(not any("installed; " in x for x in s.lines(data)), "r5.2: an install line")
        check_n_unchanged(before, "r5.2")
        say("r5.2: U8 while the other engine ran; Quit closed the app; nothing installed; the other engine untouched")
        cli.kill()
    finally:
        win.close()


def ready_then(data, name, how, hooks=()):
    """N running with N+1 ready; then the test hooks set and the update started by `how` (click | quit). Returns the
    installer's Seen."""
    for h in hooks:
        (HOOKS / h).write_text("")
    publish()
    win = window(data)
    try:
        s.wait_start(data, 0)
        banner(win, "An update is ready.")
        seen = installer_seen()
        if how == "click":
            install_click(win, data)
        else:
            quit_from_tray(data)
        until(f"{name}: the app handed over to the installer", 180, lambda: not s.app_pids())
    finally:
        win.close()
    return seen


def scenario_refuse(name, how):
    """r5.4: the installer held at PREINSTALL's start, an engine started meanwhile → PREINSTALL refuses (exit 2), N's
    files byte-equal; a click's /R starts N again with U5d; a Quit doesn't, and the next open shows U5d once."""
    data = install_n(name)
    before = install_files()
    (HOOKS / "hold").write_text("")
    publish()
    win = window(data)
    try:
        s.wait_start(data, 0)
        banner(win, "An update is ready.")
        seen = Seen(installer_pids)
        if how == "click":
            install_click(win, data)
        else:
            quit_from_tray(data)
        until(f"{name}: the installer at the barrier", 180, lambda: (HOOKS / "waiting").exists())
    finally:
        win.close()
    until(f"{name}: the launcher gone", 60, lambda: not s.app_pids())
    cli = cli_engine(name, INST / "deltabadger-engine.exe")
    cli_up(cli, name)
    pid = until(f"{name}: the installer's process", 30, lambda: seen.value and seen.value[0])
    watch = s.ExitWatch(pid)
    (HOOKS / "hold").unlink()
    code = watch.wait(120)
    check(code == 2, f"{name}: PREINSTALL's exit code {code}")
    check(not (INST / ".updating").exists(), f"{name}: the refusal left .updating")
    say(f"{name}: PREINSTALL refused with the engine running (exit 2)")
    cli.kill()
    cli.wait(30)
    starts = len(version_starts(data, V["n"]))
    if how == "click":
        until(f"{name}: N relaunched by /R", 60, lambda: s.app_pids())
    else:
        no_app_within(10, name)
        launch(data)
    check_runs(data, V["n"], f"{name}: N", before=starts)
    until(f"{name}: U5d", 60, lambda: state_lines(data, 'kind="install"'))
    check(any("didn't install this version; shown once" in x for x in s.lines(data)), f"{name}: no attempt line")
    check_n_unchanged(before, name)
    if how == "quit":
        s.cleanup()
        shown, starts = len(state_lines(data, 'kind="install"')), len(version_starts(data, V["n"]))
        launch(data)
        check_runs(data, V["n"], f"{name}: the open after", before=starts)
        time.sleep(5)
        check(len(state_lines(data, 'kind="install"')) == shown, f"{name}: U5d shown twice")
        say(f"{name}: U5d once; the next open shows nothing")


def scenario_refuse_click(name):
    scenario_refuse(name, "click")


def scenario_refuse_quit(name):
    scenario_refuse(name, "quit")


def scenario_u13(name):
    """While the installer copies (held by PREINSTALL's test Sleep with .updating written): an engine started then exits
    with the line; a second launch shows U13 and starts no engine; after POSTINSTALL N+1 serves."""
    data = install_n(name)
    seen = ready_then(data, name, "quit", hooks=("sleep",))
    until(f"{name}: .updating", 60, lambda: (INST / ".updating").exists())
    cli = cli_engine(name, INST / "deltabadger-engine.exe")
    check(cli.wait(30) == 1, f"{name}: the engine started during the copy exited {cli.returncode}")
    check(BEING_UPDATED in cli.out.read_text(errors="replace"), f"{name}: {cli.out.read_text(errors='replace')[-500:]}")
    say(f"{name}: an engine started while .updating exists exits 1: {BEING_UPDATED}")
    win = window(data)
    try:
        until(f"{name}: U13 in the log", 60, lambda: any(x == "desktop: showing being-updated" for x in s.lines(data)))
        status_text(win, "U13", "Deltabadger is being updated. Open it again in a moment.")
        check(not s.engine_pids(), f"{name}: an engine runs: {s.engine_pids()}")
        say(f"{name}: the second launch shows U13; no engine")
    except Fail:
        if not (INST / ".updating").exists():
            raise Fail(f"{name}: the installer finished before the second launch looked (no U13 to see)")
        raise
    finally:
        win.close()
    code = wait_installer_exit(name, seen)
    check(code == 0, f"{name}: the installer exited {code}")
    check(not (INST / ".updating").exists(), f"{name}: .updating after POSTINSTALL")
    check(not s.engine_pids(), f"{name}: an engine ran before POSTINSTALL")
    s.cleanup()
    launch(data)
    check_runs(data, V["next"], f"{name}: after POSTINSTALL")
    # The start during the copy didn't settle the attempt: N+1 settles it as installed, with no U5d.
    until(f"{name}: the attempt settled", 30, lambda: any("the installer installed this version" in x for x in s.lines(data)))
    time.sleep(5)
    u5d = state_lines(data, 'kind="install"')
    check(not u5d, f"{name}: U5d after a good install: {u5d}")


def scenario_u10(name):
    """An installer that ends after writing .updating (PREINSTALL's test Quit) → the next launch shows U10 and starts no
    engine; an engine started by hand exits with the line."""
    data = install_n(name)
    seen = ready_then(data, name, "quit", hooks=("quit",))
    wait_installer_exit(name, seen)
    check((INST / ".updating").exists(), f"{name}: no .updating left")
    cli = cli_engine(name, INST / "deltabadger-engine.exe")
    check(cli.wait(30) == 1 and BEING_UPDATED in cli.out.read_text(errors="replace"), f"{name}: the CLI engine ran")
    win = window(data)
    try:
        status_text(win, "U10", "Deltabadger didn't start: its last update didn't finish. Download Deltabadger from the download page and install it again. Your data is kept.")
        win.poll("U10's download link", 10, "var e = document.getElementById('link'); return e && !e.hidden;")
        check(any(x == "desktop: showing update-unfinished" for x in s.lines(data)), f"{name}: no U10 line")
        time.sleep(5)
        check(not s.engine_pids(), f"{name}: an engine runs")
        say(f"{name}: U10 with its download link, no engine")
    finally:
        win.close()


def scenario_killed(name):
    """The installer killed at 0.5 s steps (5 runs): each outcome is N, N+1, U10 or no launcher, never an engine of a mixed
    pair; and one killed before PREINSTALL (held at its barrier) → the next open runs N with U5d."""
    n_pair = None
    outcomes = []
    for k in range(1, 6):
        data = install_n(f"{name}-{k}")
        n_pair = n_pair or installed()
        seen = ready_then(data, f"{name}-{k}", "quit")
        until(f"{name}-{k}: the installer", 60, lambda: seen.at)
        while time.monotonic() < seen.at + 0.5 * k:
            time.sleep(0.01)
        s.run(["taskkill", "/F", "/PID", str(seen.value[0])])
        time.sleep(1)
        pair = installed()
        try:
            launch(data)
        except OSError as e:
            # A Deltabadger.exe the kill left half written (D-DESKTOP-WIN-INTERRUPTED-COPY): Windows won't start it.
            exe = INST / "Deltabadger.exe"
            outcomes.append("the launcher doesn't start")
            say(f"{name}-{k}: killed {0.5 * k:.1f} s in → Deltabadger.exe doesn't start ({e}; {exe.stat().st_size if exe.exists() else 'no'} bytes)")
            check(not s.engine_pids(), f"{name}-{k}: an engine runs")
            s.cleanup()
            continue
        time.sleep(30)
        lines = s.lines(data)[(version_starts(data, V["n"]) + version_starts(data, V["next"]) or [0])[-1]:]
        engines = s.engine_pids()
        if "desktop: showing update-unfinished" in lines:
            outcome = "U10"
            check(not engines, f"{name}-{k}: U10 with an engine")
        elif engines:
            check(pair in (n_pair, next_pair()), f"{name}-{k}: an engine runs from a mixed pair")
            outcome = "N+1 runs" if is_next() else "N runs"
        elif not s.app_pids():
            outcome = "the launcher doesn't start"
        else:
            raise Fail(f"{name}-{k}: no outcome: {lines[-10:]}")
        outcomes.append(outcome)
        say(f"{name}-{k}: killed {0.5 * k:.1f} s in → {outcome}")
        s.cleanup()
    say(f"killed installers: {outcomes}")

    data = install_n(f"{name}-before")
    before = install_files()
    ready_then(data, f"{name}-before", "quit", hooks=("hold",))
    until(f"{name}-before: at the barrier", 60, lambda: (HOOKS / "waiting").exists())
    for pid in installer_pids():
        s.kill(pid)
    (HOOKS / "hold").unlink()
    check(not (INST / ".updating").exists(), "killed before PREINSTALL wrote .updating")
    starts = len(version_starts(data, V["n"]))
    launch(data)
    check_runs(data, V["n"], f"{name}-before", before=starts)
    until(f"{name}-before: U5d", 60, lambda: state_lines(data, 'kind="install"'))
    check_n_unchanged(before, f"{name}-before")
    say(f"{name}-before: killed before PREINSTALL → N with U5d")


def scenario_deb(name):
    """The .deb: U6 in the banner, no package requested, bundle_type() = Deb."""
    data = data_dir(name)
    shutil.rmtree(data, ignore_errors=True)
    data.mkdir(parents=True)
    publish()
    win = window(data)
    try:
        s.wait_start(data, 0)
        got = banner(win, "An update is available. Quit Deltabadger")
        check(got["line"] == "An update is available. Quit Deltabadger, then download the new .deb file from the download page and open it to install it.", got["line"])
        check("desktop: bundle Some(Deb), APPIMAGE unset" in s.lines(data), "bundle_type() is not Deb")
        check(any(r[0] == "latest.json" for r in Feed.requests), "no feed request")
        check(not package_gets(), f"a .deb requested the package: {package_gets()}")
        say("U6 shown; the feed read, no package requested; bundle_type() = Deb")
    finally:
        win.close()


SCENARIOS = {
    "p1-p2-p5": scenario_p1_p2_p5, "p4-p6": scenario_p4_p6, "p3": scenario_p3, "p7": scenario_p7, "p8": scenario_p8,
    "lock": scenario_lock, "refuse-click": scenario_refuse_click, "refuse-quit": scenario_refuse_quit, "u13": scenario_u13,
    "u10": scenario_u10, "killed": scenario_killed, "deb": scenario_deb,
}


def cleanup():
    for pid in installer_pids():
        s.kill(pid)
    s.cleanup()


def main():
    global ARGS, P, V
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")  # Windows' console codepage can't print "→"
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser()
    ap.add_argument("--kind", required=True, choices=["nsis", "appimage", "deb"])
    ap.add_argument("--packages", required=True, type=lambda p: Path(p).resolve())
    ap.add_argument("--work", required=True, type=lambda p: Path(p).resolve())
    ap.add_argument("scenarios", nargs="+", choices=list(SCENARIOS))
    ARGS = ap.parse_args()
    P, V = ARGS.packages, json.loads((ARGS.packages / "versions.json").read_text())
    check(os.environ.get("CI") == "true", "runs on a CI runner only (CI=true)")
    ARGS.work.mkdir(parents=True, exist_ok=True)
    app = INST / "Deltabadger.exe" if WIN else Path("/usr/bin/Deltabadger") if ARGS.kind == "deb" else ARGS.work / "app"
    s.ARGS = argparse.Namespace(kind=ARGS.kind, app=app, work=ARGS.work, fixture_2x=None)
    s.scenario_env = env_for
    if LINUX:
        import ctypes
        check(ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0, "PR_SET_CHILD_SUBREAPER")
    server = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    say(f"N = {V['n']}, N+1 = {V['next']}; feed on http://127.0.0.1:{PORT}/latest.json")

    results = []
    for name in ARGS.scenarios:
        say(f"=== {name} ===")
        t0, Feed.requests, Feed.dir, Feed.throttle = time.monotonic(), [], None, 0
        try:
            SCENARIOS[name](name)
            outcome = "passed"
        except Exception as e:  # every scenario runs; the run fails at the end
            outcome = f"FAILED: {e}"
            traceback.print_exc()
            print(f"feed requests: {Feed.requests}")
            for f in sorted(ARGS.work.glob("cli-*.out")):
                print(f"::group::{f}\n{f.read_text(errors='replace')[-5000:]}\n::endgroup::")
            for d in sorted({data_dir(name), *ARGS.work.glob(f"{name}*/data")}):
                for f in ("deltabadger.log", "deltabadger-stderr.log"):
                    if (d / "log" / f).exists():
                        print(f"::group::{d / 'log' / f}\n{(d / 'log' / f).read_text(errors='replace')[-20000:]}\n::endgroup::")
        finally:
            cleanup()
        results.append((name, outcome, time.monotonic() - t0))
        say(f"=== {name}: {outcome} ({results[-1][2]:.0f} s) ===")
    summary = [f"| {ARGS.kind} updates | {n} | {o} | {t:.0f} s |" for n, o, t in results]
    print("\n".join(summary))
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as f:
            f.write("| leg | scenario | outcome | time |\n|---|---|---|---|\n" + "\n".join(summary) + "\n")
    server.shutdown()
    sys.exit(0 if all(o == "passed" for _, o, _ in results) else 1)


if __name__ == "__main__":
    main()
