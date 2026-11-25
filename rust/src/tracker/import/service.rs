//! PLAN-710-confirm: an import's one shared operation. A fresh upload is read and either written at once or parked with
//! its question: one record per upload (`import_parked_uploads`), its bytes in Rust's own directory
//! (`<data>/import-parked/<token>`), and one opaque handle per option the page shows (`import_parked_choices`), each
//! standing for 8a's sealed answer and what it was issued for. A confirmation writes only that file to the destinations
//! its handle stands for, at most once: the claim, the write and the outcome commit in one transaction (`run::write_in`),
//! every transition is conditional on the upload's state and generation, and each token's work runs in a worker that holds
//! the token's lock until its outcome commits. The web renders an `Outcome`; the MCP adapter serializes it.
use super::run::{self, Answer, Confirm, Kind};
use crate::sync::SyncError;
use crate::web::{App, WebError};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

/// A parked upload nobody answered for expires after an hour.
pub const TTL: chrono::Duration = chrono::Duration::hours(1);

/// What an import operation answers.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The one write: its counts, the venues no account takes, and (exchange, account, rows) per account written.
    Written { imported: usize, duplicates: usize, skipped: usize, unrecognised: Vec<String>, unrouted: Vec<String>, generic: usize, destinations: Vec<(i64, i64, usize)> },
    /// A question: the upload's token and format, run's question with each choice's value its handle, and each choice's
    /// kind (a venue selector's options `Select`/`Authorize`; the page's Cancel is offered on every other question). Rule
    /// M7: where its rows would go, and every account its warnings name, by id.
    Question { token: String, format: &'static str, answer: Answer, kinds: Vec<Kind>, destinations: Vec<run::Destination>, warning_accounts: Vec<WarningAccount> },
    /// Cancel on an open question: nothing written, the upload gone.
    Cancelled,
    /// A named Binance identity refusal; the upload is closed with it.
    Refused(String),
    /// `error_<key>` before anything was written ("unreadable", "malformed", "unknown_timezone", "no_account_for_format").
    Error(&'static str),
    /// No usable upload: none sent, or a token that is not an open upload of this user, or its bytes changed (rule K2).
    NoFile,
    /// The request's sign-in ended before anything was read or written.
    SignedOut,
}

/// Rule M7: an account a question's warnings name: its id, exchange, label and (where the warning counts them) rows.
#[derive(Debug, Clone, PartialEq)]
pub struct WarningAccount { pub account_id: i64, pub exchange_id: i64, pub label: crate::identity::Label, pub rows: Option<usize> }

/// The live accounts of `user` each warning of `answer` names, in the order named (a prospective account has no id yet).
fn warning_accounts(c: &Connection, user: i64, answer: &Answer) -> Result<Vec<WarningAccount>, WebError> {
    let Answer::Warned(warnings, _) = answer else { return Ok(vec![]) };
    let mut named: Vec<(&crate::identity::Label, Option<usize>)> = vec![];
    for w in warnings {
        match w {
            run::Warning::HeldElsewhere(l) | run::Warning::OtherAccount(l) => named.push((l, None)),
            run::Warning::Existing(per) | run::Warning::Destination(per) => named.extend(per.iter().map(|(l, n)| (l, Some(*n)))),
            run::Warning::Undone(_, ls) => named.extend(ls.iter().map(|l| (l, None))),
            _ => {}
        }
    }
    let live: Vec<(i64, i64)> = c.prepare("SELECT id, exchange_id FROM exchange_accounts WHERE user_id = ?1 AND removed_at IS NULL ORDER BY id").map_err(db)?
        .query_map([user], |r| Ok((r.get(0)?, r.get(1)?))).map_err(db)?.collect::<Result<_, _>>().map_err(db)?;
    let mut labels = vec![];
    for (id, exchange) in live { labels.push((id, exchange, crate::identity::label(c, id).map_err(db)?)); }
    let mut out: Vec<WarningAccount> = vec![];
    for (label, rows) in named {
        let Some((id, exchange, _)) = labels.iter().find(|(_, _, l)| l == label) else { continue };
        if !out.iter().any(|a| a.account_id == *id && a.rows == rows) { out.push(WarningAccount { account_id: *id, exchange_id: *exchange, label: label.clone(), rows }); }
    }
    Ok(out)
}

/// Whether the request's sign-in still holds, asked inside the operation's transaction.
pub type SignedIn = Arc<dyn Fn(&Connection) -> Result<bool, SyncError> + Send + Sync>;

/// Test hooks: a token whose operation stops at a point, as a process killed there would (`before_commit`: the joint
/// transaction is dropped; `before_cleanup`: the outcome committed, its bytes and handles left).
#[doc(hidden)]
pub static CRASH: Mutex<Vec<(String, &'static str)>> = Mutex::new(Vec::new());
fn crash(token: &str, point: &str) -> bool { CRASH.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|(t, p)| t == token && *p == point) }

/// Rule K4: one lock per token. An entry goes only while the map holds its only reference, so no second lock for one
/// token ever exists.
static LOCKS: Mutex<Option<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = Mutex::new(None);
fn lock_of(token: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut map = LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
    map.get_or_insert_with(HashMap::new).entry(token.to_string()).or_default().clone()
}
fn prune_locks() {
    let mut map = LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(map) = map.as_mut() { map.retain(|_, lock| Arc::strong_count(lock) > 1); }
}

/// `work` in a worker that owns the token's lock until it returns: a dropped request cannot release it early, and the
/// worker still runs to its outcome.
async fn worker(app: &App, token: &str, work: impl FnOnce(&App, &Connection) -> Result<Outcome, WebError> + Send + 'static) -> Result<Outcome, WebError> {
    let lock = lock_of(token);
    let app = app.clone();
    let done = tokio::spawn(async move {
        let _guard = lock.lock_owned().await;
        let inner = app.clone();
        app.db(move |c| work(&inner, c)).await
    }).await.map_err(|e| WebError::Task(e.to_string()));
    prune_locks();
    done?
}

/// The upload record's `TOKEN = /\A[a-f0-9]{16}\z/`.
pub fn token_shaped(t: &str) -> bool { t.len() == 16 && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) }
/// A handle: `<token>.<gen>.<n>.<128-bit random, hex>`.
pub fn handle_shaped(h: &str) -> bool {
    let parts: Vec<&str> = h.split('.').collect();
    matches!(parts.as_slice(), [t, g, n, r] if token_shaped(t) && !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit()) && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
        && r.len() == 32 && r.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// Rule L6: Rust's own parking directory, beside the install's database (2.x never opens it).
pub fn park_dir(app: &App, c: &Connection) -> PathBuf {
    match c.path().filter(|p| !p.is_empty()).and_then(|p| std::path::Path::new(p).parent().map(|d| d.to_path_buf())) {
        Some(dir) => dir.join("import-parked"),
        None => app.config.tmp_dir.join("import-parked"),
    }
}

fn sha256(bytes: &[u8]) -> String { use sha2::Digest; hex::encode(sha2::Sha256::digest(bytes)) }
fn db(e: impl std::fmt::Display) -> WebError { WebError::Config(format!("import: {e}")) }
fn time(t: chrono::DateTime<chrono::Utc>) -> String { crate::sync::sql_time(t) }

/// A fresh upload: written at once, or parked with its question (generation 1).
pub async fn upload(app: &App, user: i64, format: &'static str, bytes: Vec<u8>, filename: Option<String>, signed_in: SignedIn) -> Result<Outcome, WebError> {
    // #resolve_offset: a Binance file's zone is the one its own name states.
    let zone = match run::offset(format, filename.as_deref(), &bytes) {
        None => None,
        Some(Some(zone)) => Some(zone),
        Some(None) => return Ok(Outcome::Error("unknown_timezone")),
    };
    let token = format!("{:016x}", rand::random::<u64>());
    // Read on a blocking thread of its own; the shared connection is taken only for the write.
    let (read_bytes, read_zone) = (bytes.clone(), zone.clone());
    let file = match tokio::task::spawn_blocking(move || run::read(format, &read_bytes, read_zone.as_deref())).await.map_err(|e| WebError::Task(e.to_string()))? {
        Ok(file) => file,
        Err(answer) => return Ok(early(answer)),
    };
    let inner = app.clone();
    let outcome = app.db(move |c| -> Result<Outcome, WebError> {
        let app = inner;
        let tx = crate::engine::model::immediate(c)?;
        if !signed_in(&tx).map_err(|e| db(e.0))? { return Ok(Outcome::SignedOut); }
        // Rule M5: the clock is read inside the write lock.
        let now = app.now();
        let ran = match run::write_in(&tx, &app.cipher, user, format, file, &token, &Confirm::default(), now) {
            Ok(ran) => ran,
            Err(error) => return match run::refusal(&error) { Some(reason) => Ok(Outcome::Refused(reason)), None => Err(db(error.0)) },
        };
        let outcome = match ran.answer {
            Answer::Ambiguous(_) | Answer::Warned(..) => {
                let dir = park_dir(&app, &tx);
                std::fs::create_dir_all(&dir).map_err(db)?;
                std::fs::write(dir.join(&token), &bytes).map_err(db)?;
                tx.execute("INSERT INTO import_parked_uploads (token, user_id, sha256, bytes_len, format, filename, zone, gen, state, expires_at, picks, created_at) \
                            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 'open', ?8, '[]', ?9)",
                           params![token, user, sha256(&bytes), bytes.len() as i64, format, filename, zone, time(now + TTL), time(now)]).map_err(db)?;
                issue(&tx, user, &token, 1, format, ran)?
            }
            answer => early(answer),
        };
        tx.commit().map_err(db)?;
        Ok(outcome)
    }).await?;
    sweep(app).await;
    Ok(outcome)
}

/// An answer that parks nothing.
fn early(answer: Answer) -> Outcome {
    match answer {
        Answer::Written { imported, duplicates, skipped, unrecognised, unrouted, generic, destinations } => Outcome::Written { imported, duplicates, skipped, unrecognised, unrouted, generic, destinations },
        Answer::Error(e) => Outcome::Error(e),
        Answer::Refused(reason) => Outcome::Refused(reason),
        Answer::Ambiguous(_) | Answer::Warned(..) => Outcome::NoFile,
    }
}

/// Rule K3: one handle per option of generation `gen`, standing for its sealed answer and what it was issued for; the
/// question with each value its handle.
fn issue(c: &Connection, user: i64, token: &str, gen: i64, format: &'static str, ran: run::Ran) -> Result<Outcome, WebError> {
    let run::Ran { mut answer, issued, destinations } = ran;
    if !matches!(answer, Answer::Ambiguous(_) | Answer::Warned(..)) { return Ok(early(answer)); }
    let warning_accounts = warning_accounts(c, user, &answer)?;
    let (Answer::Ambiguous(choices) | Answer::Warned(_, choices)) = &mut answer else { unreachable!() };
    let mut kinds = vec![];
    for (n, choice) in choices.iter_mut().enumerate() {
        let i = issued.get(n).cloned().unwrap_or(run::Issued { kind: Kind::Accept, route: None, outside: None });
        let handle = format!("{token}.{gen}.{n}.{}", hex::encode(rand::random::<[u8; 16]>()));
        let kind = match i.kind { Kind::Select => "select", Kind::Authorize => "authorize", Kind::Accept => "accept" };
        let dest = serde_json::json!({ "chosen": run::posted(choice), "value": choice.value, "venue": choice.venue, "key": choice.key, "route": i.route, "outside": i.outside });
        c.execute("INSERT INTO import_parked_choices (handle, token, gen, kind, dest_json) VALUES (?1, ?2, ?3, ?4, ?5)", params![handle, token, gen, kind, dest.to_string()]).map_err(db)?;
        choice.value = handle;
        kinds.push(i.kind);
    }
    Ok(Outcome::Question { token: token.to_string(), format, answer, kinds, destinations, warning_accounts })
}

/// The open upload `token` of `user` (rule K1: open and unexpired, read under the token's lock and the write lock).
struct Parked { format: &'static str, zone: Option<String>, gen: i64, sha256: String, bytes_len: i64, picks: serde_json::Value }
fn parked(c: &Connection, token: &str, user: i64, now: chrono::DateTime<chrono::Utc>) -> Result<Option<Parked>, WebError> {
    let row: Option<(String, Option<String>, i64, String, i64, String)> = c.query_row(
        "SELECT format, zone, gen, sha256, bytes_len, picks FROM import_parked_uploads WHERE token = ?1 AND user_id = ?2 AND state = 'open' AND expires_at > ?3",
        params![token, user, time(now)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).optional().map_err(db)?;
    Ok(row.and_then(|(format, zone, gen, sha256, bytes_len, picks)| {
        let format = run::FORMATS.iter().copied().find(|f| *f == format)?;
        Some(Parked { format, zone, gen, sha256, bytes_len, picks: serde_json::from_str(&picks).unwrap_or_else(|_| serde_json::json!([])) })
    }))
}

/// Rule K2: the parked bytes, exactly as uploaded: `bytes_len` bytes, their raw sha256, and nothing after them.
fn verified(path: &std::path::Path, p: &Parked) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut bytes = vec![0; usize::try_from(p.bytes_len).ok()?];
    file.read_exact(&mut bytes).ok()?;
    let mut more = [0u8; 1];
    if file.read(&mut more).ok()? != 0 { return None; }
    (sha256(&bytes) == p.sha256).then_some(bytes)
}

/// A conditional transition (rule L5): exactly one row moves, or nothing does.
fn transition(c: &Connection, token: &str, from: &str, to: &str, gen: i64) -> Result<bool, WebError> {
    Ok(c.execute("UPDATE import_parked_uploads SET state = ?3 WHERE token = ?1 AND state = ?2 AND gen = ?4", params![token, from, to, gen]).map_err(db)? == 1)
}

/// A confirmation: the handle's sealed answer written, at most once, or the question asked again (generation + 1).
pub async fn confirm(app: &App, user: i64, token: String, handle: String, signed_in: SignedIn) -> Result<Outcome, WebError> {
    let key = token.clone();
    worker(app, &key, move |app, c| confirm_locked(app, c, user, &token, &handle, &signed_in)).await
}

fn confirm_locked(app: &App, c: &Connection, user: i64, token: &str, handle: &str, signed_in: &SignedIn) -> Result<Outcome, WebError> {
    let tx = crate::engine::model::immediate(c)?;
    if !signed_in(&tx).map_err(|e| db(e.0))? { return Ok(Outcome::SignedOut); }
    let now = app.now();
    let Some(p) = parked(&tx, token, user, now)? else { return Ok(Outcome::NoFile) };
    let dir = park_dir(app, &tx);
    let Some(bytes) = verified(&dir.join(token), &p) else {
        // D-IMPORT-FILE-IMMUTABLE: changed bytes are uploaded again; no generation is issued for them.
        transition(&tx, token, "open", "error", p.gen)?;
        tx.commit().map_err(db)?;
        cleanup(c, &dir, token);
        return Ok(Outcome::NoFile);
    };
    let file = match run::read(p.format, &bytes, p.zone.as_deref()) {
        Ok(file) => file,
        Err(_) => { transition(&tx, token, "open", "error", p.gen)?; tx.commit().map_err(db)?; cleanup(c, &dir, token); return Ok(Outcome::NoFile) }
    };
    // Rules K3/L3: the handle's sealed answer, of this upload and this generation; any other is stale (asked again).
    let choice: Option<(i64, String, String)> = tx.query_row("SELECT gen, kind, dest_json FROM import_parked_choices WHERE handle = ?1 AND token = ?2", params![handle, token],
                                                             |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(db)?;
    let picked = p.picks.as_array().and_then(|a| a.iter().rev().find_map(|v| v["venue"].as_i64()));
    let choice = choice.filter(|(gen, _, _)| *gen == p.gen).map(|(_, kind, dest)| (kind, serde_json::from_str::<serde_json::Value>(&dest).unwrap_or_default()));
    let mut picks = p.picks.clone();
    let (chosen, route, outside) = match &choice {
        Some((_, d)) => (d["chosen"].as_str().map(str::to_string), d["route"].as_str().map(str::to_string), d["outside"].as_str().map(str::to_string)),
        None => (None, None, None),
    };
    let select = choice.as_ref().is_some_and(|(kind, _)| kind == "select");
    // Rule M6: each explicit click is kept with the upload (a venue picked; an account picked).
    if let (Some((kind, d)), Some(list)) = (&choice, picks.as_array_mut()) {
        if kind == "select" { list.push(serde_json::json!({ "venue": d["venue"] })); } else { list.push(serde_json::json!({ "answer": d["value"] })); }
    }
    let confirm = Confirm { chosen: chosen.as_deref(), stale: choice.is_none(), select, route: route.as_deref(), outside: outside.as_deref(),
                            picked: if select { choice.as_ref().and_then(|(_, d)| d["venue"].as_i64()) } else { picked } };
    // Rule M2: the claim, the write and the outcome in one transaction.
    if !transition(&tx, token, "open", "claimed", p.gen)? { return Ok(Outcome::NoFile); }
    match run::write_in(&tx, &app.cipher, user, p.format, file, token, &confirm, now) {
        Ok(ran) => match ran.answer {
            Answer::Ambiguous(_) | Answer::Warned(..) => {
                // Rule P5: a new question replaces the old generation in the same commit; only its handles resolve.
                let gen = p.gen + 1;
                let moved = tx.execute("UPDATE import_parked_uploads SET state = 'open', gen = ?3, picks = ?4 WHERE token = ?1 AND state = 'claimed' AND gen = ?2",
                                       params![token, p.gen, gen, picks.to_string()]).map_err(db)?;
                if moved != 1 { return Ok(Outcome::NoFile); }
                tx.execute("DELETE FROM import_parked_choices WHERE token = ?1", [token]).map_err(db)?;
                let outcome = issue(&tx, user, token, gen, p.format, ran)?;
                if crash(token, "before_commit") { return Err(db("crashed before the commit")); }
                tx.commit().map_err(db)?;
                Ok(outcome)
            }
            answer => {
                let state = if matches!(answer, Answer::Written { .. }) { "written" } else { "error" };
                if !transition(&tx, token, "claimed", state, p.gen)? { return Ok(Outcome::NoFile); }
                if crash(token, "before_commit") { return Err(db("crashed before the commit")); }
                tx.commit().map_err(db)?;
                if crash(token, "before_cleanup") { return Err(db("crashed before the cleanup")); }
                cleanup(c, &dir, token);
                Ok(early(answer))
            }
        },
        Err(error) => match run::refusal(&error) {
            // Rule N6: a refusal closes the upload in the commit that observes it (write_in kept nothing).
            Some(reason) => {
                if !transition(&tx, token, "claimed", "refused", p.gen)? { return Ok(Outcome::NoFile); }
                if crash(token, "before_commit") { return Err(db("crashed before the commit")); }
                tx.commit().map_err(db)?;
                cleanup(c, &dir, token);
                Ok(Outcome::Refused(reason))
            }
            None => Err(db(error.0)),
        },
    }
}

/// Cancel on an open question of `user`: `cancelled` commits under the token's lock, so a confirmation queued behind it
/// finds the upload closed.
pub async fn cancel(app: &App, user: i64, token: String, signed_in: SignedIn) -> Result<Outcome, WebError> {
    let key = token.clone();
    worker(app, &key, move |app, c| {
        let tx = crate::engine::model::immediate(c)?;
        if !signed_in(&tx).map_err(|e| db(e.0))? { return Ok(Outcome::SignedOut); }
        let now = app.now();
        let Some(p) = parked(&tx, &token, user, now)? else { return Ok(Outcome::NoFile) };
        if !transition(&tx, &token, "open", "cancelled", p.gen)? { return Ok(Outcome::NoFile); }
        tx.commit().map_err(db)?;
        cleanup(c, &park_dir(app, c), &token);
        Ok(Outcome::Cancelled)
    }).await
}

/// After a terminal state committed: the bytes and the handles go, and the record (a later post finds nothing open, as it
/// would find it closed). May fail without harm: the sweep repeats it.
fn cleanup(c: &Connection, dir: &std::path::Path, token: &str) {
    let _ = std::fs::remove_file(dir.join(token));
    let gone = || -> rusqlite::Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM import_parked_choices WHERE token = ?1 AND EXISTS (SELECT 1 FROM import_parked_uploads WHERE token = ?1 AND state NOT IN ('open', 'claimed'))", [token])?;
        tx.execute("DELETE FROM import_parked_uploads WHERE token = ?1 AND state NOT IN ('open', 'claimed')", [token])?;
        tx.commit()
    };
    let _ = gone();
}

/// Rules K4/Q5: each open upload past its expiry is closed (`expired`, under its lock, never two at once; a busy one is
/// skipped), and every closed upload's bytes and handles go; a parked file with no record goes too.
pub async fn sweep(app: &App) {
    let inner = app.clone();
    let _ = app.db(move |c| { sweep_now(&inner, c); Ok(()) }).await;
}

/// Branch review r1 #7: a handback forgets every parked upload: its rows go in the handback's own transaction (2.x never
/// reads either table, and a 2.x portable export would copy them), its bytes right after it commits (`forget_files`).
pub fn forget_rows(c: &Connection) -> rusqlite::Result<()> {
    let present: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'import_parked_uploads')", [], |r| r.get(0))?;
    if present { c.execute_batch("DELETE FROM import_parked_choices; DELETE FROM import_parked_uploads;")?; }
    Ok(())
}
/// The parking directory beside `c`'s database, removed whole.
pub fn forget_files(c: &Connection) {
    if let Some(dir) = c.path().filter(|p| !p.is_empty()).and_then(|p| std::path::Path::new(p).parent().map(|d| d.join("import-parked"))) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// `sweep` on the caller's connection.
pub fn sweep_now(app: &App, c: &Connection) {
    let (expired, closed, dir, known) = {
        let now = time(app.now());
        let list = |sql: &str| -> Vec<String> { c.prepare(sql).and_then(|mut s| s.query_map([&now], |r| r.get(0))?.collect()).unwrap_or_default() };
        (list("SELECT token FROM import_parked_uploads WHERE state = 'open' AND expires_at <= ?1"),
         list("SELECT token FROM import_parked_uploads WHERE state NOT IN ('open', 'claimed') AND ?1 = ?1"),
         park_dir(app, c), list("SELECT token FROM import_parked_uploads WHERE ?1 = ?1"))
    };
    for token in expired {
        let lock = lock_of(&token);
        let Ok(_guard) = lock.try_lock() else { continue };
        let now = time(app.now());
        let closed = c.execute("UPDATE import_parked_uploads SET state = 'expired' WHERE token = ?1 AND state = 'open' AND expires_at <= ?2", params![token, now]).unwrap_or(0) == 1;
        if closed { cleanup(c, &dir, &token); }
    }
    for token in closed {
        let lock = lock_of(&token);
        let Ok(_guard) = lock.try_lock() else { continue };
        cleanup(c, &dir, &token);
    }
    // A file whose record never committed (or went) is no upload.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age > TTL.to_std().unwrap_or_default());
            if !known.contains(&name) && old { let _ = std::fs::remove_file(entry.path()); }
        }
    }
    prune_locks();
}
