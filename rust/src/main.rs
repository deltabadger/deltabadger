//! `deltabadger prepare`: take the engine lock, then create or upgrade this install's databases, and exit.
//! It does not claim the install (the engine does, in plan 2), so Rails can start right after it.
//! Env: STORAGE_DIR (default ./storage) and the Rails *_DATABASE_PATH variables.
use deltabadger::lease::{self, LeaseError};
use deltabadger::store::{self, Paths, StoreError, EMBEDDED};

fn main() {
    let env = |k: &str| std::env::var(k).ok();
    match std::env::args().nth(1).as_deref() {
        Some("prepare") => {
            let storage = env("STORAGE_DIR").unwrap_or_else(|| "storage".into());
            // Rails would open the database a URL names; this build only knows paths. Refusing keeps both
            // engines' lock next to the same file.
            if let Some(var) = ["DATABASE_URL", "PRIMARY_DATABASE_URL", "QUEUE_DATABASE_URL", "CACHE_DATABASE_URL", "CABLE_DATABASE_URL"].into_iter().find(|v| env(v).is_some_and(|x| !x.trim().is_empty())) {
                fail(&format!("{var} is set; this build supports only *_DATABASE_PATH"));
            }
            let paths = Paths::from_env(&env, storage.as_ref());
            let _lock = match lease::lock(&paths, chrono::Utc::now()) {
                Ok(l) => l,
                Err(LeaseError::Locked) => fail("another Deltabadger engine is running on this data"),
                Err(LeaseError::RailsAlive { seconds_ago }) => fail(&format!("the Rails app looks alive (job heartbeat {seconds_ago}s ago); stop it first")),
                Err(e) => fail(&format!("{e:?}")),
            };
            match store::open(&paths, &EMBEDDED) {
                Ok(o) if o.created => println!("created a new install in {storage}"),
                Ok(o) => println!("ready; applied {} migration(s)", o.applied_twins.len()),
                Err(StoreError::NewerSchema { unknown }) => fail(&format!("this data was upgraded by a newer version of Deltabadger ({}). Run that version or newer.", unknown.join(", "))),
                Err(StoreError::OlderSchema { missing }) => fail(&format!("this data predates what this build can upgrade ({} missing). Upgrade it with the full app first.", missing.len())),
                Err(StoreError::Unrecognised { path }) => fail(&format!("{} is not a Deltabadger database; refusing to replace it", path.display())),
                Err(e) => fail(&format!("{e:?}")),
            }
        }
        _ => println!("deltabadger {}\nusage: deltabadger prepare", env!("CARGO_PKG_VERSION")),
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("deltabadger: {msg}");
    std::process::exit(1)
}
