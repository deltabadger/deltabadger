//! Build-time inputs from the Rails app: the migration versions this build understands, the
//! translations in config/locales (src/web/i18n.rs), and the files `serve` hands out (src/web/assets.rs).
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::{env, fs, path::{Path, PathBuf}};
use yaml_rust2::parser::{Event, EventReceiver, Parser};
use yaml_rust2::scanner::TScalarStyle;

mod provenance_build_gate;

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("..");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    if let Err(error)=provenance_build_gate::check(&root){eprintln!("R4 provenance gate: {error}");std::process::exit(1);}
    accounting_gate(&root);
    migrations(&root, &out);
    locales(&root, &out);
    assets(&root, &out);
}

fn migrations(root: &Path, out: &Path) {
    let dir = root.join("db/migrate");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut versions = Vec::new();
    for entry in fs::read_dir(&dir).expect("read Rails migrations") {
        let entry = entry.expect("read migration entry");
        if !entry.file_type().expect("read migration file type").is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_str().expect("migration filename is UTF-8");
        let version = name.get(..14).expect("migration has a version prefix");
        assert!(version.bytes().all(|b| b.is_ascii_digit()), "invalid migration version: {name}");
        versions.push(version.to_owned());
    }
    versions.sort();
    let body: String = versions.iter().map(|v| format!("    {v:?},\n")).collect();
    fs::write(out.join("migrations.rs"), format!("pub const MIGRATIONS: &[&str] = &[\n{body}];\n")).unwrap();
}

/// Every file under `dir`, as (path relative to `dir` with `/` separators, full path), sorted.
fn files_under(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current).unwrap_or_else(|e| panic!("{}: {e}", current.display())) {
            let path = entry.expect("read asset entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(dir).unwrap().components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
                found.push((relative, path));
            }
        }
    }
    found.sort();
    found
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/vnd.microsoft.icon",
        "woff2" => "font/woff2",
        "webmanifest" => "application/manifest+json",
        other => panic!("no content type is listed for .{other} ({name}); add it to build.rs"),
    }
}

/// `favicon/site.webmanifest` + bytes -> `/assets/favicon/site-<16 hex of sha256>.webmanifest`.
fn fingerprinted(logical: &str, bytes: &[u8]) -> String {
    let digest: String = Sha256::digest(bytes).iter().take(8).map(|b| format!("{b:02x}")).collect();
    let (directory, file) = logical.rsplit_once('/').map_or(("", logical), |(d, f)| (d, f));
    let (stem, extension) = file.rsplit_once('.').unwrap_or((file, ""));
    let directory = if directory.is_empty() { String::new() } else { format!("{directory}/") };
    format!("/assets/{directory}{stem}-{digest}.{extension}")
}

/// The files `serve` hands out, embedded in the binary:
/// - app/assets/builds/application.{js,css} (built by script/rust/build_assets.sh) and everything under
///   app/assets/images, at fingerprinted /assets/ paths, as Sprockets publishes them;
/// - `*.erb` among the images (the web manifest), with each `<%= asset_path '...' %>` replaced by that
///   file's fingerprinted path, which is all the ERB there does;
/// - everything under public/ (fonts, service worker, error pages) at its own path, as Rails' static
///   file server does. public/assets is Sprockets' output and is skipped.
///
/// The two built files are not committed. A release build without them stops here: a binary that
/// ships must have them. Any other build (debug, tests) compiles with an empty table and
/// `BUILT = false`: the engine and its tests need neither Bun nor the web UI, `deltabadger serve`
/// refuses to start, and a test that needs the web app fails with `assets::MISSING`.
fn assets(root: &Path, out: &Path) {
    let (builds, images, public) = (root.join("app/assets/builds"), root.join("app/assets/images"), root.join("public"));
    for dir in [&builds, &images, &public] {
        println!("cargo:rerun-if-changed={}", dir.display());
    }
    let mut sources: Vec<(String, PathBuf)> = Vec::new();
    for built in ["application.js", "application.css"] {
        let path = builds.join(built);
        if !path.is_file() {
            let release = env::var("PROFILE").is_ok_and(|profile| profile == "release");
            assert!(!release, "{} is missing, and a release build embeds the web assets: run script/rust/build_assets.sh (it needs Bun), then build again", path.display());
            println!("cargo:warning=built without web assets ({built} is missing): `deltabadger serve` will not start; run script/rust/build_assets.sh");
            fs::write(out.join("assets.rs"), "pub const BUILT: bool = false;\npub static EMBEDDED: &[Embedded] = &[];\n").unwrap();
            return;
        }
        sources.push((built.to_string(), path));
    }
    sources.extend(files_under(&images).into_iter().filter(|(name, _)| !name.ends_with(".md")));

    // (url, logical name, file to embed)
    let mut embedded: Vec<(String, String, PathBuf)> = Vec::new();
    for (logical, path) in sources.iter().filter(|(name, _)| !name.ends_with(".erb")) {
        let bytes = fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        embedded.push((fingerprinted(logical, &bytes), logical.clone(), path.clone()));
    }
    for (name, path) in sources.iter().filter(|(name, _)| name.ends_with(".erb")) {
        let logical = name.trim_end_matches(".erb").to_string();
        let mut text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        while let Some(start) = text.find("<%= asset_path '") {
            let from = start + "<%= asset_path '".len();
            let length = text[from..].find("' %>").unwrap_or_else(|| panic!("{}: unterminated asset_path tag", path.display()));
            let wanted = text[from..from + length].to_string();
            let url = embedded.iter().find(|(_, l, _)| *l == wanted).map(|(u, _, _)| u.clone())
                .unwrap_or_else(|| panic!("{}: asset_path '{wanted}' names no file under app/assets/images", path.display()));
            text.replace_range(start..from + length + "' %>".len(), &url);
        }
        assert!(!text.contains("<%"), "{}: only <%= asset_path '...' %> is supported in an asset template", path.display());
        let rendered = out.join(logical.replace('/', "_"));
        fs::write(&rendered, &text).unwrap();
        embedded.push((fingerprinted(&logical, text.as_bytes()), logical, rendered));
    }
    for (name, path) in files_under(&public).into_iter().filter(|(name, _)| !name.starts_with("assets/")) {
        embedded.push((format!("/{name}"), String::new(), path));
    }
    embedded.sort();
    let body: String = embedded.iter().map(|(url, logical, path)| {
        format!("    Embedded {{ url: {url:?}, logical: {logical:?}, content_type: {:?}, body: include_bytes!({:?}) }},\n",
                content_type(url), path.canonicalize().unwrap().to_str().expect("asset path is UTF-8"))
    }).collect();
    fs::write(out.join("assets.rs"), format!("pub const BUILT: bool = true;\npub static EMBEDDED: &[Embedded] = &[\n{body}];\n")).unwrap();
}

/// config/locales/*.yml as one sorted table of ("<locale>.<full.key>", "<text>"). Files are merged in
/// name order, a later file winning, as Rails' I18n load path does. An array adds its index as a key
/// segment. Rails reads these files with Psych (YAML 1.1), where an unquoted `yes`, `no`, `on`, `off`,
/// `true` or `false` is a boolean: as a KEY it becomes `true`/`false` (base.en.yml has `yes:` and `no:`
/// under tax_report.summary, so Rails' keys there are `true` and `false`), and that is reproduced. As
/// a VALUE it would render as "true"/"false"; that, and a null, stop the build, so it gets quoted.
fn locales(root: &Path, out: &Path) {
    let dir = root.join("config/locales");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<PathBuf> = fs::read_dir(&dir).expect("read config/locales")
        .map(|e| e.expect("read locale entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "yml"))
        .collect();
    files.sort();
    let mut flat = BTreeMap::new();
    for file in &files {
        let text = fs::read_to_string(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let mut reader = Flatten::default();
        Parser::new_from_str(&text).load(&mut reader, true).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        assert!(reader.problems.is_empty(), "{}: {}", file.display(), reader.problems.join("; "));
        flat.extend(reader.flat);
    }
    let body: String = flat.iter().map(|(k, v)| format!("    ({k:?}, {v:?}),\n")).collect();
    fs::write(out.join("translations.rs"), format!("pub static TRANSLATIONS: &[(&str, &str)] = &[\n{body}];\n")).unwrap();
}

enum Frame {
    Map { path: String, key: Option<String> },
    Seq { path: String, next: usize },
}

/// One file's leaves. `anchors` maps a YAML anchor id to the path of the value it marks, so an alias
/// (`binance_us: *binance_config`) copies that value's leaves under its own path.
#[derive(Default)]
struct Flatten {
    stack: Vec<Frame>,
    flat: BTreeMap<String, String>,
    anchors: BTreeMap<usize, String>,
    problems: Vec<String>,
}

fn psych_boolean(plain: &str) -> Option<&'static str> {
    match plain.to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" => Some("true"),
        "no" | "false" | "off" => Some("false"),
        _ => None,
    }
}

impl Flatten {
    /// The full key of the value now starting, or None when the scalar just read is a mapping key.
    fn value_path(&mut self, scalar: Option<(&str, TScalarStyle)>) -> Option<String> {
        let join = |path: &str, segment: &str| if path.is_empty() { segment.to_string() } else { format!("{path}.{segment}") };
        match self.stack.last_mut() {
            None => Some(String::new()),
            Some(Frame::Seq { path, next }) => {
                *next += 1;
                Some(join(path, &(*next - 1).to_string()))
            }
            Some(Frame::Map { path, key }) => match key.take() {
                Some(k) => Some(join(path, &k)),
                None => {
                    let (text, style) = scalar?; // a mapping or sequence as a key: left unset, reported below
                    let boolean = if style == TScalarStyle::Plain { psych_boolean(text) } else { None };
                    *key = Some(boolean.unwrap_or(text).to_string());
                    None
                }
            },
        }
    }
}

impl EventReceiver for Flatten {
    fn on_event(&mut self, event: Event) {
        let mapping = matches!(event, Event::MappingStart(..));
        match event {
            Event::MappingStart(anchor, _) | Event::SequenceStart(anchor, _) => {
                let path = self.value_path(None).unwrap_or_else(|| {
                    self.problems.push("a mapping or sequence used as a key".into());
                    String::new()
                });
                if anchor != 0 {
                    self.anchors.insert(anchor, path.clone());
                }
                self.stack.push(if mapping { Frame::Map { path, key: None } } else { Frame::Seq { path, next: 0 } });
            }
            Event::MappingEnd | Event::SequenceEnd => {
                self.stack.pop();
            }
            Event::Scalar(text, style, anchor, _) => {
                let Some(path) = self.value_path(Some((&text, style))) else { return };
                if anchor != 0 {
                    self.anchors.insert(anchor, path.clone());
                }
                let null = matches!(text.as_str(), "" | "~" | "null" | "Null" | "NULL");
                if style == TScalarStyle::Plain && (null || psych_boolean(&text).is_some()) {
                    self.problems.push(format!("{path} is `{text}`, which Rails reads as a boolean or null, not text; quote it"));
                }
                self.flat.insert(path, text);
            }
            Event::Alias(anchor) => {
                let (Some(to), Some(from)) = (self.value_path(None), self.anchors.get(&anchor).cloned()) else {
                    self.problems.push("an alias used as a key, or to an unknown anchor".into());
                    return;
                };
                let below = format!("{from}.");
                let copied: Vec<(String, String)> = self.flat.iter()
                    .filter(|(k, _)| **k == from || k.starts_with(&below))
                    .map(|(k, v)| (format!("{to}{}", &k[from.len()..]), v.clone()))
                    .collect();
                self.flat.extend(copied);
            }
            _ => {}
        }
    }
}

// R6 is enforced on every Cargo build, including Ruby-free CI and release builds.
fn accounting_gate(root: &Path) {
    for path in ["rust/src", "script/rust/histories_timestamp_gate.py", "script/rust/histories_accounting_gate.py", "script/rust/histories_arithmetic_primitives.json"] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    match std::process::Command::new("python3").arg(root.join("script/rust/histories_accounting_gate.py")).status() {
        Ok(status) if status.success() => {},
        result => { eprintln!("R6 shared arithmetic gate failed: {result:?}"); std::process::exit(1); },
    }
}
