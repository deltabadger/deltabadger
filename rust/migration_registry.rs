//! Rust's own schema migrations: `<dir>/<14-digit version>_<name>.sql`. Shared by build.rs (which embeds
//! rust/migrations and stops the build on any problem here) and store (whose tests load fixture directories).
use std::path::Path;

/// (version, name, path) of every migration in `dir`, sorted by version, after checking that each file is named
/// `<version>_<name>.sql`, that no version repeats or equals one of `rails_versions`, and that no file controls the
/// transaction it runs in (the migrator runs each file in its own). A missing directory has none; dotfiles are skipped.
pub fn load(dir: &Path, rails_versions: &[String]) -> Result<Vec<(String, String, std::path::PathBuf)>, String> {
    let mut found = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        other => other.map_err(|e| format!("{}: {e}", dir.display()))?,
    };
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        let file = path.file_name().and_then(|f| f.to_str()).ok_or_else(|| format!("{}: not a UTF-8 name", path.display()))?.to_string();
        if file.starts_with('.') { continue; }
        let parsed = file.strip_suffix(".sql").and_then(|stem| stem.split_once('_'))
            .filter(|(version, name)| version.len() == 14 && version.bytes().all(|b| b.is_ascii_digit()) && !name.is_empty());
        let Some((version, name)) = parsed else { return Err(format!("{file}: a Rust migration is named <14-digit version>_<name>.sql")) };
        if rails_versions.iter().any(|v| v == version) { return Err(format!("{file}: version {version} is a Rails migration's")); }
        let sql = std::fs::read_to_string(&path).map_err(|e| format!("{file}: {e}"))?;
        if let Some(statement) = transaction_control(&sql) { return Err(format!("{file}: `{statement}` controls the transaction; the migrator runs each file in its own")); }
        found.push((version.to_string(), name.to_string(), path));
    }
    found.sort();
    if let Some(pair) = found.windows(2).find(|pair| pair[0].0 == pair[1].0) {
        return Err(format!("version {} is used by two Rust migrations", pair[0].0));
    }
    Ok(found)
}

/// The first statement that begins with BEGIN, COMMIT, ROLLBACK, SAVEPOINT or RELEASE, ignoring comments and quoted
/// text. A bare END is left out: it also closes a trigger body (`… BEGIN …; END;`).
// ponytail: a keyword scan, not a parser; store::apply also refuses a file that leaves its transaction (END included).
fn transaction_control(sql: &str) -> Option<String> {
    let mut code = String::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '-' if chars.peek() == Some(&'-') => { for c in chars.by_ref() { if c == '\n' { break; } } code.push(' '); }
            '/' if chars.peek() == Some(&'*') => { chars.next(); let mut last = ' '; for c in chars.by_ref() { if last == '*' && c == '/' { break; } last = c; } code.push(' '); }
            '\'' | '"' | '`' | '[' => { let close = if c == '[' { ']' } else { c }; for d in chars.by_ref() { if d == close { break; } } code.push_str(" x "); }
            c => code.push(c),
        }
    }
    code.split(';')
        .filter_map(|statement| statement.split(|c: char| !c.is_ascii_alphabetic()).find(|w| !w.is_empty()))
        .map(str::to_ascii_uppercase)
        .find(|word| ["BEGIN", "COMMIT", "ROLLBACK", "SAVEPOINT", "RELEASE"].contains(&word.as_str()))
}
