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
/// app's `version`, which is what Umbrel compares to offer an Update, is `web`'s tag (Umbrel's convention: they move
/// together, PLAN-B5 §5).
#[test]
fn every_umbrel_image_is_pinned_and_the_version_is_webs_tag() {
    let mut web_tag = None;
    for (name, service) in services() {
        let Some(image) = service["image"].as_str() else { continue };
        let tag = pinned_tag(image).unwrap_or_else(|| panic!("{name}: {image} is not pinned by tag and digest"));
        if name == "web" { web_tag = Some(tag.to_string()) }
    }
    let web_tag = web_tag.expect("a web service with an image");
    assert_eq!(load("umbrel-app.yml")["version"].as_str(), Some(web_tag.as_str()), "umbrel-app.yml's version is web's tag");
}

/// Red on purpose while `web` runs a prerelease (`3.0.0-beta.N`): this app reaches every Umbrel install once merged, so
/// it may only merge pinned to a released image (PLAN-B5 §5, §9 item 2). The final commit repins to `v3.0.0`'s index.
#[test]
fn the_umbrel_app_ships_only_a_released_image() {
    let web = services().into_iter().find(|(n, _)| n == "web").expect("web").1;
    let tag = pinned_tag(web["image"].as_str().unwrap()).expect("pinned").to_string();
    assert!(!tag.contains('-'), "web runs the prerelease {tag}: repin to a released image before this merges");
}

/// The services Umbrel runs: `app_proxy` (Umbrel's), `web`, and the one-shot `upgrade` step before it (PLAN-B5 §1.1,
/// §3). No `jobs`: Umbrel's update stops it with the old compose and nothing re-creates it (§2). The step runs
/// docker/behind-guard.rb verbatim under 2.70.21's Ruby, never restarts, and `web` starts only once it exited 0; `web`
/// gets 60 s to stop (§4).
#[test]
fn the_umbrel_app_runs_the_step_then_web_alone() {
    let services = services();
    let names: Vec<&str> = services.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["app_proxy", "web", "upgrade"]);
    let service = |name: &str| services.iter().find(|(n, _)| n == name).unwrap().1.clone();
    let (web, upgrade) = (service("web"), service("upgrade"));

    let guard = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../docker/behind-guard.rb")).unwrap();
    assert!(!guard.contains('$'), "docker/behind-guard.rb must have no $: compose would interpolate it");
    let command: Vec<&str> = upgrade["command"].as_vec().expect("a command list").iter().filter_map(Yaml::as_str).collect();
    assert_eq!(command, ["bundle", "exec", "ruby", "-e", guard.as_str()], "upgrade runs docker/behind-guard.rb verbatim");
    assert_eq!(upgrade["restart"].as_str(), Some("no"));
    assert!(pinned_tag(upgrade["image"].as_str().unwrap()).is_some_and(|t| t == "2.70.21"), "the step is 2.70.21's");
    let env: Vec<&str> = upgrade["environment"].as_hash().unwrap().keys().filter_map(Yaml::as_str).collect();
    assert_eq!(env, ["SECRET_KEY_BASE"], "2.70.21's paths are web's; its RAILS_ENV is the image's");

    assert_eq!(web["depends_on"]["upgrade"]["condition"].as_str(), Some("service_completed_successfully"));
    assert_eq!(web["stop_grace_period"].as_str(), Some("60s"));
    assert_eq!(web["restart"].as_str(), Some("on-failure"));
    assert_eq!(web["command"].as_str(), Some("web"));
    assert_eq!(web["environment"]["DELTABADGER_PLATFORM"].as_str(), Some("umbrel"));
    assert_eq!(web["environment"]["APP_ROOT_URL"].as_str(), Some("http://${DEVICE_DOMAIN_NAME}:3737"));
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
