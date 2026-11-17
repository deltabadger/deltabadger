//! The engine lock with no 2.x and no queue database, so it runs on every platform. On Windows `File::try_lock` is
//! LockFileEx, which is per handle: two handles in one process conflict just as flock's do on Unix, so this test is
//! the runtime proof there that the lock excludes.
use chrono::Utc;
use deltabadger::lease::{lock, LeaseError};
use deltabadger::store::Paths;

fn paths(dir: &std::path::Path) -> Paths {
    // The queue file is never created, so the 2.x heartbeat check in `lock` is skipped.
    Paths { primary: dir.join("primary.sqlite3"), queue: dir.join("queue.sqlite3") }
}

#[test]
fn the_exclusive_lock_excludes_and_drop_frees_it() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    let now = Utc::now();

    let held = lock(&p, now).expect("the first lock is taken");
    assert!(matches!(lock(&p, now), Err(LeaseError::Locked)), "(a) a second exclusive lock is refused");

    drop(held);
    let _again = lock(&p, now).expect("(c) dropping the holder frees the lock");
}

#[test]
fn a_shared_hold_on_another_handle_blocks_the_exclusive_lock() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    std::fs::create_dir_all(p.lock_file().parent().unwrap()).unwrap();
    let v2 = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(p.lock_file()).unwrap(); // a 2.x process
    v2.try_lock_shared().unwrap();
    assert!(matches!(lock(&p, Utc::now()), Err(LeaseError::Locked)), "(b) a shared hold blocks the exclusive lock");

    drop(v2);
    lock(&p, Utc::now()).expect("(c) released, the exclusive lock succeeds");
}
