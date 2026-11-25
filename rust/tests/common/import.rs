//! PLAN-710-confirm: an import question's options are opaque handles (`tracker::import::service`). Tests read a page as
//! the values its handles stand for, and post back the handle of the value they mean.
use rusqlite::Connection;

fn handle_pattern() -> regex::Regex { regex::Regex::new(r"[0-9a-f]{16}\.[0-9]+\.[0-9]+\.[0-9a-f]{32}").unwrap() }

/// `body` with every handle replaced by the 8a value it stands for (as `dest_json.value`); a handle with no row stays.
pub fn decode(c: &Connection, body: &str) -> String {
    handle_pattern().replace_all(body, |m: &regex::Captures| {
        c.query_row("SELECT json_extract(dest_json, '$.value') FROM import_parked_choices WHERE handle = ?1", [&m[0]], |r| r.get::<_, String>(0))
            .unwrap_or_else(|_| m[0].to_string())
    }).into_owned()
}

/// The handle of the current question of `token` that stands for `value`.
pub fn handle_for(c: &Connection, token: &str, value: &str) -> Option<String> {
    c.query_row("SELECT h.handle FROM import_parked_choices h JOIN import_parked_uploads u ON u.token = h.token AND u.gen = h.gen \
                 WHERE h.token = ?1 AND json_extract(h.dest_json, '$.value') = ?2", [token, value], |r| r.get(0)).ok()
}

/// The handles a page renders, in order.
pub fn handles(body: &str) -> Vec<String> { handle_pattern().find_iter(body).map(|m| m.as_str().to_string()).collect() }

/// The token a question page parks its upload under.
pub fn token(body: &str) -> Option<String> {
    regex::Regex::new(r#"name="token" id="token" value="([0-9a-f]{16})""#).unwrap().captures(body).map(|m| m[1].to_string())
}

/// `decode` over the app's own connection.
pub async fn decoded(app: &deltabadger::web::App, body: String) -> String {
    app.db(move |c| Ok(decode(c, &body))).await.unwrap()
}

/// `handle_for` over the app's own connection.
pub async fn handle_of(app: &deltabadger::web::App, token: String, value: String) -> Option<String> {
    app.db(move |c| Ok(handle_for(c, &token, &value))).await.unwrap()
}

/// The install's parking directory (rule L6), beside its database.
pub fn parked(c: &Connection, token: &str) -> std::path::PathBuf {
    std::path::Path::new(c.path().unwrap()).parent().unwrap().join("import-parked").join(token)
}
