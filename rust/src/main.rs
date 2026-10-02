//! `deltabadger check | run | handback | serve | resolve-placement | decide`.
//! - check: take the engine lock, check the Rails-prepared install read-only, and exit.
//! - run: take the install over from Rails and trade its eligible bots until SIGTERM/SIGINT.
//! - handback: settle every unresolved order, then return the install to Rails.
//! - serve: take the engine lock, check the install, and serve the web UI until stopped. It runs no engine.
//!
//! Env: STORAGE_DIR (default ./storage), DATABASE_PATH and QUEUE_DATABASE_PATH; run, handback and serve also need
//! SECRET_KEY_BASE (and ACTIVE_RECORD_ENCRYPTION_* where the instance sets them). serve also reads PORT (default 3000),
//! APP_ROOT_URL, FORCE_SSL, BEHIND_PROXY and MARKET_DATA_URL. Rails creates and migrates the databases.
use deltabadger::crypto::{Cipher, EncryptionKeys};
use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::{handover, log, EngineError, SystemClock};
use deltabadger::lease::{self, EngineLock, LeaseError};
use deltabadger::store::{self, Paths, StoreError};
use deltabadger::venue::alpaca::{self, LiveFactory};

const EXIT_REFUSED: i32 = 1;
const EXIT_ENGINE_ERROR: i32 = 2;

fn main() {
    let env = |k: &str| std::env::var(k).ok();
    match std::env::args().nth(1).as_deref() {
        Some("check") => {
            refuse_url_overrides(&env);
            let paths = paths(&env);
            let _lock = take_lock(&paths);
            if let Err(e) = store::check(&paths) { fail(&explain(e)); }
            let c = rusqlite::Connection::open_with_flags(&paths.primary, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap_or_else(|e| fail(&format!("{e}")));
            match deltabadger::engine::eligibility::check_install(&c) {
                Ok(r) if r.problems.is_empty() && r.unreadable.is_empty() => println!("ready: {} bot(s) this engine can run", r.eligible.len()),
                Ok(r) if r.problems.is_empty() => fail(&format!("unreadable bot rows: {:?}", r.unreadable)),
                Ok(r) => fail(&format!("this install uses things only the full app runs:\n{}", r.problems.join("\n"))),
                Err(e) => fail(&format!("{e:?}")),
            }
        }
        Some("run") => std::process::exit(run_engine(&env)),
        Some("handback") => std::process::exit(hand_back(&env)),
        Some("serve") => {
            refuse_url_overrides(&env);
            let paths = paths(&env);
            // Held until the process exits: neither Rails nor `run` can use this install while the web UI serves it.
            let _lock = take_lock(&paths);
            let opened = store::open(&paths).unwrap_or_else(|e| fail(&explain(e)));
            let port = match env("PORT").filter(|p| !p.trim().is_empty()) {
                Some(port) => port.trim().parse::<u16>().unwrap_or_else(|_| fail("PORT must be a port number")),
                None => 3000,
            };
            let config = deltabadger::web::Config::from_env(&env).unwrap_or_else(|e| fail(&web_problem(e)));
            let app = deltabadger::web::App::new(config, &env, opened.primary, std::sync::Arc::new(SystemClock)).unwrap_or_else(|e| fail(&web_problem(e)));
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
            rt.block_on(deltabadger::web::server::serve(app, port)).unwrap_or_else(|e| fail(&web_problem(e)));
        }
        Some("resolve-placement") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            let bot_id: i64 = args.first().and_then(|a| a.parse().ok()).unwrap_or_else(|| fail("usage: deltabadger resolve-placement <bot_id> --placed <txid> | --not-placed"));
            let resolution = match (args.get(1).map(String::as_str), args.get(2)) {
                (Some("--placed"), Some(txid)) => deltabadger::engine::placement::OperatorResolution::Placed(txid.clone()),
                (Some("--not-placed"), None) => deltabadger::engine::placement::OperatorResolution::NotPlaced,
                _ => fail("usage: deltabadger resolve-placement <bot_id> --placed <txid> | --not-placed"),
            };
            refuse_url_overrides(&env);
            let storage = env("STORAGE_DIR").unwrap_or_else(|| "storage".into());
            let paths = Paths::from_env(&env, storage.as_ref());
            let _lock = lease::lock(&paths, chrono::Utc::now()).unwrap_or_else(|e| fail(&format!("{e:?}")));
            let o = store::open(&paths).unwrap_or_else(|e| fail(&format!("{e:?}")));
            deltabadger::engine::placement::resolve_by_operator(&o.primary, bot_id, resolution, chrono::Utc::now())
                .unwrap_or_else(|e| fail(&format!("{e:?}")));
            println!("resolved");
        }
        Some("decide") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let out = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                ["run", dir] => rt.block_on(deltabadger::parity::decide(std::path::Path::new(dir))).unwrap_or_else(|e| fail(&format!("{e:?}"))),
                ["plan", src, tickers, out, now] => {
                    let tickers = std::fs::read_to_string(tickers).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| fail("tickers.json is unreadable"));
                    let now = now.parse().unwrap_or_else(|_| fail("<now> must be RFC 3339, e.g. 2026-09-10T00:00:00Z"));
                    let n = deltabadger::parity::plan_copy(std::path::Path::new(src), &tickers, std::path::Path::new(out), now).unwrap_or_else(|e| fail(&format!("{e:?}")));
                    serde_json::json!({ "planned": n })
                }
                _ => fail("usage: deltabadger decide run <scenario_dir> | decide plan <src> <tickers.json> <out> <now>"),
            };
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        }
        _ => println!(
            "deltabadger {}\nusage: deltabadger check | run | handback | serve | resolve-placement <bot_id> --placed <order_id> | --not-placed | decide run <dir> | decide plan <src> <tickers.json> <out> <now>",
            env!("CARGO_PKG_VERSION")
        ),
    }
}

fn paths(env: &dyn Fn(&str) -> Option<String>) -> Paths {
    let storage = env("STORAGE_DIR").unwrap_or_else(|| "storage".into());
    Paths::from_env(env, storage.as_ref())
}

fn take_lock(paths: &Paths) -> EngineLock {
    match lease::lock(paths, chrono::Utc::now()) {
        Ok(l) => l,
        Err(LeaseError::Locked) => fail("another Deltabadger engine is running on this data"),
        Err(LeaseError::RailsAlive { seconds_ago }) => fail(&format!("the Rails app looks alive (job heartbeat {seconds_ago}s ago); stop it first")),
        Err(e) => fail(&format!("{e:?}")),
    }
}

fn explain(e: StoreError) -> String {
    match e {
        StoreError::Missing { path } => format!("{} does not exist yet: start the Rails app once to set up this install", path.display()),
        StoreError::Unrecognised { path } => format!("{} is not a Deltabadger database; refusing to touch it", path.display()),
        StoreError::Behind { missing } => format!("this data is behind this build ({} migration(s) missing): run the matching Rails app until its background jobs have finished, stop it, then start this again", missing.len()),
        StoreError::Unsupported { unknown } => format!("this data has migrations this build does not know ({}); use a newer build", unknown.join(", ")),
        StoreError::Diverged { .. } => "this data's migration history differs from this build's; refusing".into(),
        StoreError::Incompatible { problems } => format!("this data's structure differs from what this build expects:\n{}", problems.join("\n")),
        e => format!("{e:?}"),
    }
}

/// The shared start of `run` and `handback`: no URL overrides, the instance's SECRET_KEY_BASE, the exclusive lock, and an
/// install this build accepts. Refusals here exit 1 before any file is created or changed.
fn open_install(env: &dyn Fn(&str) -> Option<String>) -> (EngineLock, store::Opened, Cipher) {
    refuse_url_overrides(env);
    let secret = env("SECRET_KEY_BASE").filter(|s| !s.trim().is_empty()).unwrap_or_else(|| fail("SECRET_KEY_BASE is not set: use the instance's own"));
    let keys = EncryptionKeys::resolve(env, &secret).unwrap_or_else(|e| fail(&format!("encryption keys: {e:?}")));
    let paths = paths(env);
    let lock = take_lock(&paths);
    let opened = store::open(&paths).unwrap_or_else(|e| fail(&explain(e)));
    (lock, opened, Cipher::new(&keys))
}

fn run_engine(env: &dyn Fn(&str) -> Option<String>) -> i32 {
    let (lock, o, cipher) = open_install(env);
    if let Err(problems) = alpaca::preflight(&o.primary, &cipher) {
        eprintln!("deltabadger: refusing to take this install over:\n{}", problems.join("\n"));
        return EXIT_REFUSED;
    }
    let t = match handover::take_over(&lock, &o, &cipher, env!("CARGO_PKG_VERSION"), chrono::Utc::now()) {
        Ok(t) => t,
        Err(EngineError::Ineligible(p)) => { eprintln!("deltabadger: this install uses things only the full app runs:\n{}", p.join("\n")); return EXIT_REFUSED; }
        Err(e) => { eprintln!("deltabadger: takeover failed: {e:?}"); return EXIT_ENGINE_ERROR; }
    };
    log(&format!("took over ({:?}): {} bot(s), {} Rails job(s) removed, {} bot(s) back to scheduled",
                 t.claim, t.eligible.len(), t.deleted_jobs, t.normalised));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
    rt.block_on(async move {
        let engine = Engine::new(o.primary, LiveFactory::new(), cipher, lock);
        engine.stop_handle().on_signals();
        log("running: SIGTERM finishes the tick in hand and stops; then run `deltabadger handback` before starting Rails");
        match run::run(engine, &SystemClock).await {
            Err(EngineError::Stopped) => { log("stopped on request"); 0 }
            Err(e) => { log(&format!("engine stopped: {e:?}")); EXIT_ENGINE_ERROR }
            Ok(never) => match never {},
        }
    })
}

fn hand_back(env: &dyn Fn(&str) -> Option<String>) -> i32 {
    let (lock, o, cipher) = open_install(env);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
    // The factory and the wait live inside the runtime; the process start and the wait are hand_back_cli's.
    let result = rt.block_on(async { handover::hand_back_cli(&lock, &o, &LiveFactory::new(), &cipher, &SystemClock).await });
    let code = handover::handback_exit_code(&result);
    match result {
        Ok(n) => log(&format!("handed back: {n} bot(s) scheduled; the Rails app may start now")),
        Err(EngineError::Unresolved(ids)) => {
            eprintln!("deltabadger: handback refused: the venue could not yet account for the order of bot(s) {ids:?}.\n\
                       Look each order up on the venue's own site by its client order id (bots.transient_data.rust_placement.cl_ord_id),\n\
                       then run `deltabadger resolve-placement <bot_id> --placed <order_id>` or `--not-placed`, and run handback again.");
        }
        Err(e) => eprintln!("deltabadger: handback failed: {e:?}"),
    }
    code
}

/// Rails would open the database a URL names; this build only knows paths. Refusing keeps both engines' lock next to
/// the same file, and keeps a command from settling work against an install the operator did not mean.
fn refuse_url_overrides(env: &dyn Fn(&str) -> Option<String>) {
    if let Some(var) = ["DATABASE_URL", "PRIMARY_DATABASE_URL", "QUEUE_DATABASE_URL", "CACHE_DATABASE_URL", "CABLE_DATABASE_URL"]
        .into_iter()
        .find(|v| env(v).is_some_and(|x| !x.trim().is_empty()))
    {
        fail(&format!("{var} is set; this build supports only *_DATABASE_PATH"));
    }
}

fn web_problem(e: deltabadger::web::WebError) -> String {
    match e {
        deltabadger::web::WebError::Config(message) => message,
        other => format!("{other:?}"),
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("deltabadger: {msg}");
    std::process::exit(1)
}
