//! Rust's own schema migrations: `<dir>/<14-digit version>_<name>.sql`. Shared by build.rs (which embeds
//! rust/migrations and stops the build on any problem here) and store (whose tests load fixture directories).
use std::path::Path;

/// (version, name, path) of every migration in `dir`, sorted by version, after checking that each file is named
/// `<version>_<name>.sql`, that no version repeats or equals one of `v2_versions`, and that no file controls the
/// transaction it runs in (the migrator runs each file in its own). A missing directory has none; dotfiles are skipped.
pub fn load(dir: &Path, v2_versions: &[String]) -> Result<Vec<(String, String, std::path::PathBuf)>, String> {
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
        if v2_versions.iter().any(|v| v == version) { return Err(format!("{file}: version {version} is a Rails migration's")); }
        found.push((version.to_string(), name.to_string(), path));
    }
    found.sort();
    if let Some(pair) = found.windows(2).find(|pair| pair[0].0 == pair[1].0) {
        return Err(format!("version {} is used by two Rust migrations", pair[0].0));
    }
    // In version order: a DROP TABLE may name only a table an earlier file or statement created.
    let mut created = std::collections::BTreeSet::new();
    for (version, name, path) in &found {
        let file = format!("{version}_{name}.sql");
        let sql = std::fs::read_to_string(path).map_err(|e| format!("{file}: {e}"))?;
        if let Some(statement) = transaction_control(&sql) { return Err(format!("{file}: `{statement}` controls the transaction; the migrator runs each file in its own")); }
        check_tables(&sql, &mut created).map_err(|e| format!("{file}: {e}"))?;
    }
    Ok(found)
}

/// D-B11-9-DROP-OWN-TABLE. Every `ALTER TABLE` is refused, and `DROP TABLE` of anything but a bare name (unquoted,
/// unqualified, no IF EXISTS) that an earlier file (`created`, which this extends) or an earlier statement of this one
/// created with a plain `CREATE TABLE <name>`: so a migration replaces only Rust's own tables (create the new one, copy,
/// drop the old), never a Rails-born one, and a missing table fails the migration loudly. Returns each dropped table
/// (lower-cased) and whether this file created it (a hop table). A plain CREATE fails on an existing table, so only a
/// table the migration really made is authorised; CREATE TEMP, IF NOT EXISTS and quoted names authorise nothing.
// ponytail: a token scan, not a parser; a trigger body's statements are scanned as statements, which only refuses more.
pub fn check_tables(sql: &str, created: &mut std::collections::BTreeSet<String>) -> Result<Vec<(String, bool)>, String> {
    let mut here = std::collections::BTreeSet::new();
    let mut drops = Vec::new();
    let statements = statements(sql);
    // SQLite reads a non-ASCII character or `$` as part of a name, skips a byte-order mark between tokens and reads a
    // bind parameter (`$x(…)` included) as one token: a scan that misreads any of them could authorise the wrong table.
    // Rust's migrations use none of them outside comments and quoted text.
    if let Some(c) = statements.iter().flatten().find_map(|t| match t { Token::Punct(c) if !c.is_ascii() || "$?:@#".contains(*c) => Some(*c), _ => None }) {
        return Err(format!("{c:?} outside comments and quoted text is not allowed (a non-ASCII character or a bind parameter)"));
    }
    for statement in statements {
        let word = |i: usize| match statement.get(i) { Some(Token::Word(w)) => Some(w.to_ascii_uppercase()), _ => None };
        let bare = |i: usize| match (statement.get(i), statement.get(i + 1)) {
            (Some(Token::Word(w)), next) if !w.starts_with(|c: char| c.is_ascii_digit()) && next != Some(&Token::Punct('.')) => Some(w.to_ascii_lowercase()),
            _ => None,
        };
        match (word(0).as_deref(), word(1).as_deref()) {
            (Some("ALTER"), Some("TABLE")) => return Err(format!("ALTER TABLE is not allowed (`{}`): a migration replaces a table instead (CREATE TABLE the new one, copy its rows, DROP TABLE the old one)", text(&statement))),
            (Some("DROP"), Some("TABLE")) if word(2).as_deref() == Some("IF") =>
                return Err(format!("DROP TABLE IF EXISTS is not allowed (`{}`): a table a migration replaces must be there", text(&statement))),
            (Some("DROP"), Some("TABLE")) => match bare(2).filter(|name| statement.len() == 3 && created.contains(name)) {
                Some(name) => { created.remove(&name); drops.push((name.clone(), here.remove(&name))); }
                None => return Err(format!("DROP TABLE of a table no earlier Rust migration created (`{}`): a migration drops only a bare name it or an earlier one made with CREATE TABLE <name>", text(&statement))),
            },
            (Some("CREATE"), Some("TABLE")) if word(2).as_deref() != Some("IF") => if let Some(name) = bare(2) { created.insert(name.clone()); here.insert(name); },
            _ => {}
        }
    }
    Ok(drops)
}

/// The first statement that begins with BEGIN, COMMIT, ROLLBACK, SAVEPOINT or RELEASE, ignoring comments and quoted
/// text. A bare END is left out: it also closes a trigger body (`… BEGIN …; END;`).
// ponytail: a keyword scan, not a parser; store::apply also refuses a file that leaves its transaction (END included).
fn transaction_control(sql: &str) -> Option<String> {
    statements(sql).into_iter().filter_map(|tokens| match tokens.into_iter().next() { Some(Token::Word(w)) => Some(w.to_ascii_uppercase()), _ => None })
        .find(|word| ["BEGIN", "COMMIT", "ROLLBACK", "SAVEPOINT", "RELEASE"].contains(&word.as_str()))
}

#[derive(Debug, PartialEq)]
enum Token {
    /// A run of `[A-Za-z0-9_]`, as written.
    Word(String),
    /// A quoted string or name ('…', "…", `…`, […]), as written.
    Quoted(String),
    Punct(char),
}

/// A statement's tokens, for a refusal's text.
fn text(statement: &[Token]) -> String {
    statement.iter().map(|t| match t { Token::Word(w) | Token::Quoted(w) => w.clone(), Token::Punct(c) => c.to_string() }).collect::<Vec<_>>().join(" ")
}

/// Each statement's tokens, comments left out; statements end at a `;` outside quotes. A non-ASCII character is a
/// token of its own, as are `$ ? : @ #` (`check_tables` refuses both).
fn statements(sql: &str) -> Vec<Vec<Token>> {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = vec![Vec::new()];
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        let tokens = out.last_mut().unwrap();
        match c {
            '-' if chars.peek() == Some(&'-') => { for c in chars.by_ref() { if c == '\n' { break; } } }
            '/' if chars.peek() == Some(&'*') => { chars.next(); let mut last = ' '; for c in chars.by_ref() { if last == '*' && c == '/' { break; } last = c; } }
            '\'' | '"' | '`' | '[' => {
                let close = if c == '[' { ']' } else { c };
                let mut quoted = c.to_string();
                for d in chars.by_ref() { quoted.push(d); if d == close { break; } }
                tokens.push(Token::Quoted(quoted));
            }
            ';' => out.push(Vec::new()),
            c if word(c) => {
                let mut name = c.to_string();
                while let Some(&d) = chars.peek().filter(|d| word(**d)) { name.push(d); chars.next(); }
                tokens.push(Token::Word(name));
            }
            c if c.is_ascii_whitespace() => {}
            c => tokens.push(Token::Punct(c)),
        }
    }
    out
}
