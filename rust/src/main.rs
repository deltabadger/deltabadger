//! `deltabadger check | run | handback | serve | sync | resolve-placement | decide`.
//! - check: take the engine lock, check the Rails-prepared install read-only, and exit.
//! - run: take the install over from Rails and trade its eligible bots until SIGTERM/SIGINT.
//! - handback: settle every unresolved order, then return the install to Rails. It refuses while `run` or `serve` runs.
//! - serve: `run` with the web UI in the same process: it takes the install over and trades its eligible bots while
//!   serving the UI, until SIGTERM/SIGINT stops both. Like `run`, it needs a `handback` before Rails starts again.
//! - sync ledger|balances [<api_key_id>]: one tracker sync by hand, under the engine lock, and exit.
//!
//! Env: STORAGE_DIR (default ./storage), DATABASE_PATH and QUEUE_DATABASE_PATH; run, handback and serve also need
//! SECRET_KEY_BASE (and ACTIVE_RECORD_ENCRYPTION_* where the instance sets them). serve also reads PORT (default 3000),
//! APP_ROOT_URL, FORCE_SSL, BEHIND_PROXY and MARKET_DATA_URL. run and serve send the bots' mail and read what Rails
//! reads for it: SMTP_ADDRESS, SMTP_PORT, SMTP_DOMAIN, SMTP_USER_NAME, SMTP_PASSWORD, NOTIFICATIONS_SENDER, APP_ROOT_URL
//! and FORCE_SSL. Rails creates and migrates the databases.
use deltabadger::crypto::{Cipher, EncryptionKeys};
use deltabadger::engine::eligibility::{check_install_at, Refusal};
use deltabadger::engine::run::Engine;
use deltabadger::engine::{handover, log, Clock, EngineError, SystemClock};
use deltabadger::jobs;
use deltabadger::lease::{self, EngineLock, LeaseError};
use deltabadger::store::{self, Paths, StoreError};
use deltabadger::supervisor::{self, Ended};
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
            if let Some(secret) = env("SECRET_KEY_BASE") {
                let keys=EncryptionKeys::resolve(&env,&secret).unwrap_or_else(|_|fail("index encryption configuration unreadable"));
                deltabadger::engine::provider::bind(&c,&Cipher::new(&keys),&env).unwrap_or_else(|_|fail("index configuration reader unavailable"));
            }
            // Every background job's last run and every reference source's age (informational, no keys needed).
            for (name, s) in deltabadger::jobs::state::all(&c).unwrap_or_default() { println!("job {name}: {}", s.describe()); }
            for line in deltabadger::engine::staleness::report(&c, chrono::Utc::now()).unwrap_or_default() { println!("reference {line}"); }
            // The words `serve` refuses with too (Refusal::message). A source with no stamp is noted, never refused.
            match check_install_at(&c, chrono::Utc::now()).map_err(Refusal::Failed) {
                Ok(report) => {
                    for note in &report.notes { println!("note: {note}"); }
                    match report.refusal() {
                        Ok(eligible) => println!("ready: {} bot(s) this engine can run", eligible.len()),
                        Err(refusal) => fail(&refusal.message()),
                    }
                }
                Err(refusal) => fail(&refusal.message()),
            }
        }
        Some("run") => std::process::exit(run_engine(&env)),
        Some("handback") => std::process::exit(hand_back(&env)),
        Some("sync") => std::process::exit(sync_by_hand(&env)),
        Some("serve") => std::process::exit(serve(&env)),
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
            "deltabadger {}\nusage: deltabadger check | run | handback | serve | sync ledger|balances [<api_key_id>] | resolve-placement <bot_id> --placed <order_id> | --not-placed | decide run <dir> | decide plan <src> <tickers.json> <out> <now>",
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

/// The shared start of `run`, `serve` and `handback`: no URL overrides, the instance's SECRET_KEY_BASE, the exclusive
/// lock, and an install this build accepts. Refusals here exit 1 before any file is created or changed.
fn open_install(env: &dyn Fn(&str) -> Option<String>) -> (EngineLock, store::Opened, Cipher) {
    refuse_url_overrides(env);
    let secret = env("SECRET_KEY_BASE").filter(|s| !s.trim().is_empty()).unwrap_or_else(|| fail("SECRET_KEY_BASE is not set: use the instance's own"));
    let keys = EncryptionKeys::resolve(env, &secret).unwrap_or_else(|e| fail(&format!("encryption keys: {e:?}")));
    let paths = paths(env);
    let lock = take_lock(&paths);
    let opened = store::open(&paths).unwrap_or_else(|e| fail(&explain(e)));
    let cipher = Cipher::new(&keys);
    deltabadger::engine::provider::bind(&opened.primary,&cipher,env).unwrap_or_else(|_| fail("index configuration reader unavailable"));
    (lock, opened, cipher)
}

/// The mail sender, ready to become a service: on its own connection (a background service never uses the engine's),
/// with what the environment says about SMTP read now. Opened before the claim, so a failure here leaves the install Rails'.
fn mail_sender(env: &dyn Fn(&str) -> Option<String>, cipher: &Cipher) -> deltabadger::mail::sender::Sender<SystemClock> {
    let own = store::open(&paths(env)).unwrap_or_else(|e| fail(&explain(e))).primary;
    deltabadger::mail::sender::Sender::new(own, cipher.clone(), env, SystemClock)
}

/// The sender as the supervisor runs it: stopped by the one stop signal, woken by any engine event (a spent funds
/// budget), and looking every few seconds besides.
fn mail_service<'a>(mail: deltabadger::mail::sender::Sender<SystemClock>, stop: &deltabadger::engine::run::Shutdown,
                    wake: tokio::sync::mpsc::UnboundedReceiver<deltabadger::engine::events::EngineEvent>) -> supervisor::Service<'a> {
    supervisor::Service { name: "mail", run: Box::pin(mail.run(stop.subscribe(), Some(wake))) }
}

/// The scheduler owns its connection, cipher and event subscription and follows the supervisor's one stop signal.
/// Construction logs nothing, preserving the takeover and running lines before any service starts.
fn scheduler_service<'a>(env: &dyn Fn(&str) -> Option<String>, paths: &Paths, engine: &mut Engine<LiveFactory>, clock: &'a dyn Clock)
    -> Result<supervisor::Service<'a>, String> {
    let secret = env("SECRET_KEY_BASE").unwrap_or_default();
    let keys = EncryptionKeys::resolve(env, &secret).map_err(|e| format!("encryption keys: {e:?}"))?;
    let own = store::open(paths).map_err(explain)?;
    let cipher = Cipher::new(&keys);
    let api = jobs::data_api::config(env, &own.primary, &cipher)?.map(jobs::data_api::DataApi::live);
    let api = std::rc::Rc::new(api);
    let mut registered = jobs::reference::shared_jobs(api.clone());
    registered.extend(deltabadger::sync::jobs::register(&own.primary, &LiveFactory::new(), api).map_err(|e| e.0)?);
    let scheduler = jobs::Scheduler::new(own.primary, cipher, registered, Some(engine.subscribe()));
    Ok(supervisor::Service { name: "scheduler", run: Box::pin(scheduler.run(engine.stop_handle().subscribe(), clock)) })
}

/// The takeover `run` and `serve` share. Refusals exit 1 before anything is claimed; a takeover that fails after the
/// claim exits 2.
fn claim_install(lock: &EngineLock, o: &store::Opened, cipher: &Cipher) -> Result<(), i32> {
    if let Err(problems) = alpaca::preflight(&o.primary, cipher) {
        eprintln!("deltabadger: refusing to take this install over:\n{}", problems.join("\n"));
        return Err(EXIT_REFUSED);
    }
    let t = match handover::take_over(lock, o, cipher, env!("CARGO_PKG_VERSION"), chrono::Utc::now()) {
        Ok(t) => t,
        Err(EngineError::Ineligible(p)) => { eprintln!("deltabadger: this install uses things only the full app runs:\n{}", p.join("\n")); return Err(EXIT_REFUSED); }
        Err(e) => { eprintln!("deltabadger: takeover failed: {e:?}"); return Err(EXIT_ENGINE_ERROR); }
    };
    log(&format!("took over ({:?}): {} bot(s), {} Rails job(s) removed, {} bot(s) back to scheduled",
                 t.claim, t.eligible.len(), t.deleted_jobs, t.normalised));
    Ok(())
}

fn run_engine(env: &dyn Fn(&str) -> Option<String>) -> i32 {
    let (lock, o, cipher) = open_install(env);
    // Held until this function returns, after the runtime's shutdown: the engine drops its own handle when it returns,
    // and a service may still be draining then.
    let _held = lock.clone();
    let mail = mail_sender(env, &cipher);
    if let Err(code) = claim_install(&lock, &o, &cipher) { return code; }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
    let code = rt.block_on(async move {
        let mut engine = Engine::new(o.primary, LiveFactory::new(), cipher, lock);
        engine.stop_handle().on_signals();
        let scheduler = match scheduler_service(env, &paths(env), &mut engine, &SystemClock) {
            Ok(s) => s,
            Err(e) => { eprintln!("deltabadger: the background scheduler could not start: {e}"); return EXIT_ENGINE_ERROR; }
        };
        let services = vec![mail_service(mail, &engine.stop_handle(), engine.subscribe()), scheduler];
        log("running: SIGTERM finishes the tick in hand and stops; then run `deltabadger handback` before starting Rails");
        // `serve`'s supervisor without the web: one "ended" rule for both commands.
        match supervisor::serve(engine, None, &SystemClock, services).await {
            Ended::Stopped => { log("stopped on request"); 0 }
            Ended::Engine(e) => { log(&format!("engine stopped: {e:?}")); EXIT_ENGINE_ERROR }
            other => { log(&format!("engine stopped: {other:?}")); EXIT_ENGINE_ERROR } // a service; never the web here
        }
    });
    // As `serve`: a blocking unit a service left running gets up to 5 s; it never holds the exit longer.
    rt.shutdown_timeout(std::time::Duration::from_secs(5));
    code
}

/// `deltabadger serve`: `run` and the web UI in one process. Every refusal, the web side's
/// included (assets, config, no admin user, the port), comes before the claim, so a `serve` that cannot serve leaves
/// the install Rails'. Exit codes as `run`: 0 stopped as asked, 1 refused before claiming, 2 ended otherwise.
fn serve(env: &dyn Fn(&str) -> Option<String>) -> i32 {
    if !deltabadger::web::assets::BUILT {
        fail(deltabadger::web::assets::MISSING);
    }
    let (lock, o, cipher) = open_install(env);
    // Held until this function returns, after the runtime's shutdown: the engine drops its own handle when it returns.
    let _held = lock.clone();
    // In `check`'s words, and before anything else prints: each line names the bot and the reason. Venue and key
    // problems stay `preflight`'s (`claim_install`, below).
    if let Err(refusal) = handover::startup_report(&o.primary, &cipher, chrono::Utc::now()).map_err(Refusal::Failed).and_then(|r| r.refusal()) {
        fail(&refusal.message());
    }
    let port = match env("PORT").filter(|p| !p.trim().is_empty()) {
        Some(port) => port.trim().parse::<u16>().unwrap_or_else(|_| fail("PORT must be a port number")),
        None => 3000,
    };
    let config = deltabadger::web::Config::from_env(env).unwrap_or_else(|e| fail(&web_problem(e)));
    // The web side's own connection; the engine keeps `o.primary`. The install passed the check a moment ago, under this lock.
    let own = store::open(&paths(env)).unwrap_or_else(|e| fail(&explain(e))).primary;
    let app = deltabadger::web::App::new(config, env, own, std::sync::Arc::new(SystemClock)).unwrap_or_else(|e| fail(&web_problem(e)));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
    let listener = rt.block_on(deltabadger::web::server::bind(&app, port)).unwrap_or_else(|e| fail(&web_problem(e)));
    let mail = mail_sender(env, &cipher);
    if let Err(code) = claim_install(&lock, &o, &cipher) { return code; }
    let code = rt.block_on(async move {
        let mut engine = Engine::new(o.primary, LiveFactory::new(), cipher, lock);
        engine.stop_handle().on_signals();
        let scheduler = match scheduler_service(env, &paths(env), &mut engine, &SystemClock) {
            Ok(s) => s,
            Err(e) => { eprintln!("deltabadger: the background scheduler could not start: {e}"); return EXIT_ENGINE_ERROR; }
        };
        let services = vec![mail_service(mail, &engine.stop_handle(), engine.subscribe()), scheduler];
        log(&format!("running, with the web UI on port {port}: SIGTERM finishes the tick in hand and stops both; \
                      then run `deltabadger handback` before starting Rails"));
        match supervisor::serve(engine, Some((app, listener)), &SystemClock, services).await {
            Ended::Stopped => { log("stopped on request"); 0 }
            Ended::Engine(e) => { log(&format!("engine stopped: {e:?}; the web UI stopped with it")); EXIT_ENGINE_ERROR }
            Ended::Web(e) => { log(&format!("the web UI stopped: {e:?}; the engine finished its tick and stopped with it")); EXIT_ENGINE_ERROR }
            Ended::Service { name, error } => { log(&format!("{name} stopped: {error}; the engine and the web UI stopped with it")); EXIT_ENGINE_ERROR }
        }
    });
    // Requests still in flight get up to 5 s for their database work; the engine has already returned.
    rt.shutdown_timeout(std::time::Duration::from_secs(5));
    code
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

/// One ledger or balance sync by hand, for one Alpaca key or for every key the nightly jobs read, exactly as the
/// scheduler's job runs it. It holds the exclusive engine lock from before it opens a database until it exits, so it
/// runs only while neither the Rails app nor another `deltabadger` process (`run`, `serve`) has this install: a sync
/// can never run against an install Rails is also syncing. It claims nothing and writes no lease: an install Rails
/// owns stays Rails' (the rows are the ones Rails' own job would write). A split that moves a bot's counter passes
/// `eligibility::guard`, as under the scheduler; nothing else it writes is the engine's business. Balances by hand
/// have no market-data source, so coins keep their last price; stocks and cash are priced. A run is held to the
/// deadline its job declares, as the scheduler's runner would hold it.
fn sync_by_hand(env: &dyn Fn(&str) -> Option<String>) -> i32 {
    use deltabadger::sync::balances::NoPrices;
    use deltabadger::jobs::{Cx, Db, Job, Outcome, Wake};
    use deltabadger::sync::jobs::{BalanceSync, LedgerSync};
    const USAGE: &str = "usage: deltabadger sync ledger|balances [<api_key_id>]";
    let args: Vec<String> = std::env::args().skip(2).collect();
    let kind = match args.first().map(String::as_str) { Some(kind @ ("ledger" | "balances")) if args.len() <= 2 => kind, _ => fail(USAGE) };
    let key = args.get(1).map(|k| k.parse::<i64>().unwrap_or_else(|_| fail(USAGE)));
    let (_lock, o, cipher) = open_install(env);
    let keys = match key {
        Some(key) => vec![key],
        None => deltabadger::sync::reading_keys(&o.primary).unwrap_or_else(|e| fail(&e.0)),
    };
    if keys.is_empty() { println!("no Alpaca key to sync"); return 0; }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
    let db = Db::new(o.primary, cipher);
    let mut code = 0;
    for key in keys {
        let job: Box<dyn Job> = match kind {
            "ledger" => Box::new(LedgerSync::new(LiveFactory::new(), key)),
            _ => Box::new(BalanceSync::new(LiveFactory::new(), std::rc::Rc::new(NoPrices), key)),
        };
        let spec = job.spec();
        let name = format!("{}:{}", spec.name, spec.scope.as_deref().unwrap_or_default());
        let outcome = rt.block_on(deltabadger::sync::jobs::run_within_deadline(job.as_ref(), Cx { db: db.clone(), clock: &SystemClock }, vec![Wake::Manual(None)]));
        let recorded = rt.block_on(db.run(move |c, _| {
            use deltabadger::jobs::state;
            let at = chrono::Utc::now();
            match &outcome {
                Outcome::Done => state::record_success(c, spec.name, spec.scope.as_deref(), at)?,
                Outcome::NothingNew => state::record_run(c, spec.name, spec.scope.as_deref(), at)?,
                Outcome::Failed(m) | Outcome::Transient(m) | Outcome::RateLimited(m) => state::record_error(c, spec.name, spec.scope.as_deref(), at, m)?,
            }
            Ok(outcome)
        }));
        let outcome = match recorded { Ok(outcome) => outcome, Err(_) => { eprintln!("deltabadger: {name}: cannot save job outcome"); code = EXIT_ENGINE_ERROR; continue; } };
        match outcome {
            Outcome::Done => println!("{name}: done"),
            // An import larger than one run reads: what was read is stored, and the next run continues.
            Outcome::NothingNew => println!("{name}: not complete yet: run it again to continue"),
            Outcome::Failed(m) | Outcome::Transient(m) | Outcome::RateLimited(m) => { eprintln!("deltabadger: {name} failed: {m}"); code = EXIT_ENGINE_ERROR; }
        }
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
