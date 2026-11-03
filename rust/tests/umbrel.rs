//! The Umbrel app (deltabadger/), read as files (PLAN-B5 §1.4, §5, r1.6). This repository is its own Umbrel app store:
//! a merge of deltabadger/ reaches every Umbrel install at its next Update, so these checks run in every ordinary test
//! run, with no Docker. The compose-running proofs are in tests/image.rs.
use yaml_rust2::{Yaml, YamlLoader};

const APP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../deltabadger/");

fn load(file: &str) -> Yaml {
    let text = std::fs::read_to_string(format!("{APP}{file}")).unwrap();
    YamlLoader::load_from_str(&text).unwrap().remove(0)
}

fn services() -> Vec<(String, Yaml)> {
    let compose = load("docker-compose.yml");
    compose["services"].as_hash().unwrap().iter().map(|(k, v)| (k.as_str().unwrap().to_string(), v.clone())).collect()
}

/// `registry/repo:tag@sha256:<64 hex>`: its tag.
fn pinned_tag(image: &str) -> Option<&str> {
    let (name, digest) = image.split_once("@sha256:")?;
    let tag = name.strip_prefix("ghcr.io/deltabadger/deltabadger:")?;
    (digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) && !tag.is_empty()).then_some(tag)
}

/// Every image is pinned by tag and digest, never `:latest` (Umbrel re-pulls `latest` on a start, PLAN-B5 r1.1), and the
/// app's `version`, which is what Umbrel compares to offer an Update, names `web`'s tag (Umbrel's convention: they move
/// together). `-N` after it marks a re-release of the same image.
#[test]
fn every_umbrel_image_is_pinned_and_the_version_names_webs_tag() {
    let mut web_tag = None;
    for (name, service) in services() {
        let Some(image) = service["image"].as_str() else { continue };
        let tag = pinned_tag(image).unwrap_or_else(|| panic!("{name}: {image} is not pinned by tag and digest"));
        if name == "web" { web_tag = Some(tag.to_string()) }
    }
    let web_tag = web_tag.expect("a web service with an image");
    let version = load("umbrel-app.yml")["version"].as_str().unwrap().to_string();
    let suffix = version.strip_prefix(&web_tag).unwrap_or_else(|| panic!("version {version} doesn't name web's tag {web_tag}"));
    assert!(suffix.is_empty() || suffix.strip_prefix('-').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
            "version {version}: web's tag {web_tag}, then nothing or -N");
}

/// What decides which install a service reads, and with which secret (PLAN-B5 §1.4): any change here makes Umbrel's
/// install unreadable (C9) or swaps it for an empty one. The app id (APP_SEED and app-data derive from it), the secret
/// line, the storage bind and the two database paths stay byte-identical; no service sets the encryption pair (it would
/// override the one in `.secrets`); app_proxy reaches `web` by umbreld's container name for it.
#[test]
fn the_umbrel_app_keeps_its_installs_identity() {
    assert_eq!(load("umbrel-app.yml")["id"].as_str(), Some("deltabadger"));
    assert_eq!(load("../umbrel-app-store.yml")["id"].as_str(), Some("deltabadger"));
    let services = services();
    let proxy = &services.iter().find(|(n, _)| n == "app_proxy").expect("app_proxy").1["environment"];
    assert_eq!(proxy["APP_HOST"].as_str(), Some("deltabadger_web_1"));
    assert_eq!(proxy["APP_PORT"].as_i64(), Some(3000));
    assert!(services.iter().any(|(n, _)| n == "web"));
    for (name, service) in services.iter().filter(|(n, _)| n != "app_proxy") {
        let env = &service["environment"];
        assert_eq!(env["SECRET_KEY_BASE"].as_str(), Some("${APP_SEED}-secret-key-base"), "{name}");
        for (key, _) in env.as_hash().into_iter().flatten() {
            assert!(!key.as_str().unwrap_or_default().starts_with("ACTIVE_RECORD_ENCRYPTION_"), "{name} sets {key:?}");
        }
        for (var, path) in [("DATABASE_PATH", "/app/storage/production.sqlite3"), ("QUEUE_DATABASE_PATH", "/app/storage/production_queue.sqlite3")] {
            assert!(env[var].is_badvalue() || env[var].as_str() == Some(path), "{name}: {var}");
        }
        let volumes: Vec<&str> = service["volumes"].as_vec().into_iter().flatten().filter_map(Yaml::as_str).collect();
        assert!(volumes.contains(&"${APP_DATA_DIR}/data/storage:/app/storage"), "{name}: {volumes:?}");
    }
}

#[test]
fn pinned_tag_reads_only_a_tag_and_a_full_digest() {
    let digest = "7c8f73518134ba6414551b3978f49bd91dacc3ff75d0e1c6aea92401b449de81";
    assert_eq!(pinned_tag(&format!("ghcr.io/deltabadger/deltabadger:2.70.21@sha256:{digest}")), Some("2.70.21"));
    assert_eq!(pinned_tag("ghcr.io/deltabadger/deltabadger:latest"), None);
    assert_eq!(pinned_tag(&format!("ghcr.io/deltabadger/deltabadger@sha256:{digest}")), None);
    assert_eq!(pinned_tag(&format!("ghcr.io/deltabadger/deltabadger:2.70.21@sha256:{}", &digest[1..])), None);
    assert_eq!(pinned_tag(&format!("ghcr.io/other/app:2.70.21@sha256:{digest}")), None);
}
