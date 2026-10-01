//! `deltabadger check`: take the engine lock, check the Rails-prepared install read-only, and exit.
//! Rails creates and migrates the databases; this command does not claim the install.
//! Env: STORAGE_DIR (default ./storage), DATABASE_PATH and QUEUE_DATABASE_PATH.
use deltabadger::lease::{self, LeaseError};
use deltabadger::store::{self, Paths, StoreError};

fn main() {
    let env = |k: &str| std::env::var(k).ok();
    match std::env::args().nth(1).as_deref() {
        Some("check") => {
            let storage = env("STORAGE_DIR").unwrap_or_else(|| "storage".into());
            // Rails would open the database a URL names; this build only knows paths. Refusing keeps both
            // engines' lock next to the same file.
            if let Some(var) = [
                "DATABASE_URL",
                "PRIMARY_DATABASE_URL",
                "QUEUE_DATABASE_URL",
                "CACHE_DATABASE_URL",
                "CABLE_DATABASE_URL",
            ]
            .into_iter()
            .find(|v| env(v).is_some_and(|x| !x.trim().is_empty()))
            {
                fail(&format!(
                    "{var} is set; this build supports only *_DATABASE_PATH"
                ));
            }
            let paths = Paths::from_env(&env, storage.as_ref());
            let _lock = match lease::lock(&paths, chrono::Utc::now()) {
                Ok(l) => l,
                Err(LeaseError::Locked) => {
                    fail("another Deltabadger engine is running on this data")
                }
                Err(LeaseError::RailsAlive { seconds_ago }) => fail(&format!(
                    "the Rails app looks alive (job heartbeat {seconds_ago}s ago); stop it first"
                )),
                Err(e) => fail(&format!("{e:?}")),
            };
            match store::check(&paths) {
                Ok(()) => {
                    let c = rusqlite::Connection::open_with_flags(&paths.primary, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                        .unwrap_or_else(|e| fail(&format!("{e}")));
                    match deltabadger::engine::eligibility::check_install(&c) {
                        Ok(r) if r.problems.is_empty() && r.unreadable.is_empty() => println!("ready: {} bot(s) this engine can run", r.eligible.len()),
                        Ok(r) if r.problems.is_empty() => fail(&format!("unreadable bot rows: {:?}", r.unreadable)),
                        Ok(r) => fail(&format!("this install uses things only the full app runs:\n{}", r.problems.join("\n"))),
                        Err(e) => fail(&format!("{e:?}")),
                    }
                },
                Err(StoreError::Missing { path }) => fail(&format!("{} does not exist yet: start the Rails app once to set up this install", path.display())),
                Err(StoreError::Unrecognised { path }) => fail(&format!("{} is not a Deltabadger database; refusing to touch it", path.display())),
                Err(StoreError::Behind { missing }) => fail(&format!("this data is behind this build ({} migration(s) missing): run the matching Rails app until its background jobs have finished, stop it, then start this again", missing.len())),
                Err(StoreError::Unsupported { unknown }) => fail(&format!("this data has migrations this build does not know ({}); use a newer build", unknown.join(", "))),
                Err(StoreError::Diverged { .. }) => fail("this data's migration history differs from this build's; refusing"),
                Err(StoreError::Incompatible { problems }) => fail(&format!("this data's structure differs from what this build expects:\n{}", problems.join("\n"))),
                Err(e) => fail(&format!("{e:?}")),
            }
        }
        Some("resolve-placement") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            let bot_id: i64 = args.first().and_then(|a| a.parse().ok()).unwrap_or_else(|| fail("usage: deltabadger resolve-placement <bot_id> --placed <txid> | --not-placed"));
            let resolution = match (args.get(1).map(String::as_str), args.get(2)) {
                (Some("--placed"), Some(txid)) => deltabadger::engine::placement::OperatorResolution::Placed(txid.clone()),
                (Some("--not-placed"), None) => deltabadger::engine::placement::OperatorResolution::NotPlaced,
                _ => fail("usage: deltabadger resolve-placement <bot_id> --placed <txid> | --not-placed"),
            };
            let storage = env("STORAGE_DIR").unwrap_or_else(|| "storage".into());
            let paths = Paths::from_env(&env, storage.as_ref());
            let _lock = lease::lock(&paths, chrono::Utc::now()).unwrap_or_else(|e| fail(&format!("{e:?}")));
            let o = store::open(&paths).unwrap_or_else(|e| fail(&format!("{e:?}")));
            deltabadger::engine::placement::resolve_by_operator(&o.primary, bot_id, resolution, chrono::Utc::now())
                .unwrap_or_else(|e| fail(&format!("{e:?}")));
            println!("resolved");
        }
        Some("decide") => {
            let dir = std::env::args().nth(3).unwrap_or_else(|| fail("usage: deltabadger decide run <scenario_dir>"));
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let out = rt.block_on(deltabadger::parity::decide(std::path::Path::new(&dir))).unwrap_or_else(|e| fail(&format!("{e:?}")));
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        }
        _ => println!(
            "deltabadger {}\nusage: deltabadger check | resolve-placement | decide run <dir>",
            env!("CARGO_PKG_VERSION")
        ),
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("deltabadger: {msg}");
    std::process::exit(1)
}
