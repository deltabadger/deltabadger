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
                Ok(()) => println!("ready"),
                Err(StoreError::Missing { path }) => fail(&format!("{} does not exist yet: start the Rails app once to set up this install", path.display())),
                Err(StoreError::Unrecognised { path }) => fail(&format!("{} is not a Deltabadger database; refusing to touch it", path.display())),
                Err(StoreError::Behind { missing }) => fail(&format!("this data is behind this build ({} migration(s) missing): run the matching Rails app until its background jobs have finished, stop it, then start this again", missing.len())),
                Err(StoreError::Unsupported { unknown }) => fail(&format!("this data has migrations this build does not know ({}); use a newer build", unknown.join(", "))),
                Err(StoreError::Diverged { .. }) => fail("this data's migration history differs from this build's; refusing"),
                Err(StoreError::Incompatible { problems }) => fail(&format!("this data's structure differs from what this build expects:\n{}", problems.join("\n"))),
                Err(e) => fail(&format!("{e:?}")),
            }
        }
        _ => println!(
            "deltabadger {}\nusage: deltabadger check",
            env!("CARGO_PKG_VERSION")
        ),
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("deltabadger: {msg}");
    std::process::exit(1)
}
