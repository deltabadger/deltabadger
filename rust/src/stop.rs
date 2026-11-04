//! The desktop's stop (PLAN-B6 §3.4, r1.7): with `DELTABADGER_STOP_ON_STDIN_EOF=1`, end-of-file on stdin is one more
//! stop source, latched once for the whole process. The desktop app closes the engine's stdin on every quit, and the pipe
//! closes by itself when the app crashes or is force-quit, so the engine stops instead of being left running. Windows has
//! no SIGTERM, and a windowless child gets no Ctrl-C from its app. Docker runs with stdin at /dev/null (EOF at once),
//! hence opt-in. The refusal page and the C7 wait also wait for this latch (`latched`); the engine's `Shutdown::on_signals`
//! registers a hook (`on_request`). Both act at once if the latch is already set.
use std::sync::OnceLock;
use tokio::sync::watch;

fn latch() -> &'static watch::Sender<bool> {
    static LATCH: OnceLock<watch::Sender<bool>> = OnceLock::new();
    LATCH.get_or_init(|| watch::channel(false).0)
}

type Hook = Box<dyn Fn() + Send>;

fn hooks() -> std::sync::MutexGuard<'static, Vec<Hook>> {
    static HOOKS: std::sync::Mutex<Vec<Hook>> = std::sync::Mutex::new(Vec::new());
    HOOKS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sets the latch and runs every hook, on the caller's thread. Never exits the process: each phase of `serve` decides
/// what a stop means there.
pub fn request() {
    latch().send_replace(true);
    for hook in hooks().iter() { hook() }
}

/// Runs `hook` when the latch is set, at once if it already is: the engine's stop flag is set from the stdin thread itself,
/// so no tick can start between the latch and a task that would have relayed it (a hook may run twice; it must not mind).
pub fn on_request(hook: impl Fn() + Send + 'static) {
    let mut hooks = hooks();
    if requested() { hook() }
    hooks.push(Box::new(hook));
}

/// Whether a stop came in. `serve` checks it between its start phases (PLAN-B6 r3.6).
pub fn requested() -> bool { *latch().borrow() }

/// Resolves once the latch is set, at once if it already is. Any runtime.
pub async fn latched() {
    let mut rx = latch().subscribe();
    let _ = rx.wait_for(|set| *set).await;
}

/// With `DELTABADGER_STOP_ON_STDIN_EOF=1`: a thread reads stdin to its end (or a read error), then sets the latch.
/// Without it, stdin is never read.
pub fn watch_stdin(env: &dyn Fn(&str) -> Option<String>) {
    if env("DELTABADGER_STOP_ON_STDIN_EOF").as_deref().map(str::trim) != Some("1") { return }
    std::thread::spawn(|| {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
        request();
    });
}

/// Tests (debug builds): with DELTABADGER_TEST_PAUSE=`point`, the caller holds there until the file DELTABADGER_TEST_RELEASE
/// names exists, after logging "paused at `point`" (PLAN-B6 r2.5, PLAN-B7 r4.7). Release builds never pause.
pub fn pause(point: &str) {
    if let Some(release) = pause_release(point) { while !release.exists() { std::thread::sleep(std::time::Duration::from_millis(20)) } }
}

/// `pause` inside the runtime, which keeps running its other tasks (the stop handlers) meanwhile.
pub async fn pause_async(point: &str) {
    if let Some(release) = pause_release(point) { while !release.exists() { tokio::time::sleep(std::time::Duration::from_millis(20)).await } }
}

fn pause_release(point: &str) -> Option<std::path::PathBuf> {
    if !cfg!(debug_assertions) || std::env::var("DELTABADGER_TEST_PAUSE").ok().as_deref() != Some(point) { return None }
    crate::engine::log(&format!("paused at {point}"));
    Some(std::env::var("DELTABADGER_TEST_RELEASE").expect("DELTABADGER_TEST_RELEASE: the file that ends the pause").into())
}
