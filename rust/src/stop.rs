//! The desktop's stop (PLAN-B6 §3.4, r1.7): with `DELTABADGER_STOP_ON_STDIN_EOF=1`, end-of-file on stdin is one more
//! stop source, latched once for the whole process. The desktop app closes the engine's stdin on every quit, and the pipe
//! closes by itself when the app crashes or is force-quit, so the engine stops instead of being left running. Windows has
//! no SIGTERM, and a windowless child gets no Ctrl-C from its app. Docker runs with stdin at /dev/null (EOF at once),
//! hence opt-in. Whoever waits for a stop (the refusal page, the C7 wait, the engine's `Shutdown::on_signals`) also
//! waits for this latch, and resolves at once if it is already set.
use std::sync::OnceLock;
use tokio::sync::watch;

fn latch() -> &'static watch::Sender<bool> {
    static LATCH: OnceLock<watch::Sender<bool>> = OnceLock::new();
    LATCH.get_or_init(|| watch::channel(false).0)
}

/// Sets the latch. Never exits the process: each phase of `serve` decides what a stop means there.
pub fn request() { latch().send_replace(true); }

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

