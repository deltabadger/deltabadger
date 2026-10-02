//! Build-time inputs from the Rails app: the migration versions this build understands, and the
//! files `serve` hands out (src/web/assets.rs).
use sha2::{Digest, Sha256};
use std::{env, fs, path::{Path, PathBuf}};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("..");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    migrations(&root, &out);
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
fn assets(root: &Path, out: &Path) {
    let (builds, images, public) = (root.join("app/assets/builds"), root.join("app/assets/images"), root.join("public"));
    for dir in [&builds, &images, &public] {
        println!("cargo:rerun-if-changed={}", dir.display());
    }
    let mut sources: Vec<(String, PathBuf)> = Vec::new();
    for built in ["application.js", "application.css"] {
        let path = builds.join(built);
        assert!(path.is_file(), "{} is missing: run script/rust/build_assets.sh first", path.display());
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
    fs::write(out.join("assets.rs"), format!("pub static EMBEDDED: &[Embedded] = &[\n{body}];\n")).unwrap();
}
