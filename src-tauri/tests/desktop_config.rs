//! The desktop's build configuration (moved from test/config/desktop_configuration_test.rb, PLAN-B6 §4.3): what the
//! bundle carries and runs before it builds, the release gate, and the platform gates in the launcher's source.
use std::path::{Path, PathBuf};

fn root() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf() }

fn read(path: &str) -> String { std::fs::read_to_string(root().join(path)).unwrap_or_else(|e| panic!("{path}: {e}")) }

fn config() -> serde_json::Value { serde_json::from_str(&read("src-tauri/tauri.conf.json")).unwrap() }

// Was: "Linux builds use the empty bundle skeleton and system Ruby fallback". No build runs Ruby now: every platform
// runs the one engine script, and no platform file overrides it.
#[test]
fn no_build_runs_ruby_and_every_platform_builds_the_engine_first() {
    let config = config();
    assert_eq!(config["build"]["beforeBuildCommand"], "bash script/rust/desktop_engine.sh");
    assert_eq!(config["build"]["beforeDevCommand"]["script"], "bash script/rust/desktop_engine.sh --debug");
    assert_eq!(config["build"]["beforeDevCommand"]["wait"], true);
    for platform in ["tauri.linux.conf.json", "tauri.windows.conf.json", "tauri.macos.conf.json"] {
        assert!(!root().join("src-tauri").join(platform).exists(), "{platform} would override the build command");
    }
    let script = read("script/rust/desktop_engine.sh");
    for ruby in ["ruby", "bundle exec", "rails", "gem "] {
        assert!(!script.contains(ruby), "desktop_engine.sh mentions {ruby:?}");
    }
}

// The bundle is the shell, the sidecar and the catalogue: nothing of Ruby or Rails.
#[test]
fn the_bundle_carries_the_engine_and_the_catalogue_and_nothing_of_rails() {
    let config = config();
    let bundle = &config["bundle"];
    assert_eq!(bundle["externalBin"], serde_json::json!(["binaries/deltabadger-engine"]));
    let resources = bundle["resources"].as_object().unwrap();
    assert_eq!(resources["../db/seed_data/catalogue.json.gz"], "db/seed_data/catalogue.json.gz");
    assert!(resources.keys().all(|k| !k.contains("_bundle")), "{resources:?}");
    assert_eq!(bundle["macOS"]["minimumSystemVersion"], "11.3");
    assert_eq!(config["build"]["frontendDist"], "ui");
    // The executable is the app's name, not the crate's: the engine finds its app by it (rust/src/bundle.rs, Windows).
    assert_eq!(config["mainBinaryName"], "Deltabadger");
    assert!(read("script/rust/desktop_engine.sh").contains("MACOSX_DEPLOYMENT_TARGET=11.3"));
}

// PLAN-B8 r1.10: an updater signature only from the real key (never the placeholder), Apple signing only with its
// secrets, and a release carries desktop assets only when both are configured. Otherwise the apps build unsigned, as
// workflow artifacts, and script/release/validate.sh refuses a stable tag (validate_test.sh proves that).
#[test]
fn releases_attach_desktop_apps_only_when_updater_and_apple_signing_are_configured() {
    let workflow = read(".github/workflows/release.yml");
    assert!(workflow.contains("[ \"$pubkey\" != REPLACE_WITH_TAURI_UPDATER_PUBLIC_KEY ] && updater_ready=true"));
    for secret in ["TAURI_SIGNING_PRIVATE_KEY", "APPLE_CERTIFICATE", "APPLE_ID", "APPLE_TEAM_ID"] {
        assert!(workflow.contains(&format!("${secret}\" ]")), "readiness does not require {secret}");
    }
    assert!(workflow.contains("echo '{\"bundle\":{\"createUpdaterArtifacts\":false}}' > \"$conf\""));
    assert!(workflow.contains("SIGNED: ${{ needs.meta.outputs.dry == 'true' || (needs.meta.outputs.updater_ready == 'true' && needs.meta.outputs.apple_ready == 'true') }}"));
    assert!(workflow.contains("if: env.DRY != 'true' && env.SIGNED == 'true'\n        run: gh release upload"));
    assert!(read("script/release/validate.sh").contains("a stable release needs B13"));
}

// The macOS-only calls broke the other platforms once already, in two places at once.
#[test]
fn platform_specific_window_apis_stay_behind_their_platform_gate() {
    for file in ["src-tauri/src/lib.rs", "src-tauri/src/supervisor.rs", "src-tauri/src/secrets.rs"] {
        let source = read(file);
        let lines: Vec<&str> = source.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            for symbol in ["ActivationPolicy", "title_bar_style", "hidden_title", "RunEvent::Reopen"] {
                if !line.contains(symbol) {
                    continue;
                }
                let preceding = lines[index.saturating_sub(5)..index].join("\n");
                assert!(
                    preceding.contains("cfg(target_os = \"macos\")") || line.contains("cfg(target_os = \"macos\")"),
                    "{symbol} on {file}:{} is not behind a macOS gate",
                    index + 1
                );
            }
        }
    }
}

// The engine's pages are a remote origin to Tauri: the app's commands reach them only through a capability that names
// them (the Settings update button is check_for_updates).
#[test]
fn the_engines_pages_may_call_the_apps_commands() {
    let capability: serde_json::Value = serde_json::from_str(&read("src-tauri/capabilities/default.json")).unwrap();
    assert_eq!(capability["remote"]["urls"], serde_json::json!(["http://127.0.0.1:*"]));
    let permissions = capability["permissions"].as_array().unwrap();
    assert!(permissions.contains(&serde_json::json!("allow-check-for-updates")), "{permissions:?}");
    assert!(read("src-tauri/build.rs").contains("\"check_for_updates\""));
}

// PLAN-B7 §7, r1.13: the committed config updates from the production feed only, refuses a package signed for another
// version, and builds the updater artifacts; nothing insecure is ever committed (the proofs pass those with --config).
#[test]
fn the_updater_config_is_the_production_one() {
    let config = config();
    let updater = &config["plugins"]["updater"];
    assert_eq!(updater["endpoints"], serde_json::json!(["https://github.com/deltabadger/deltabadger/releases/latest/download/latest.json"]));
    assert_eq!(updater["requireSignedVersion"], true);
    assert_eq!(config["bundle"]["createUpdaterArtifacts"], true);
    let text = read("src-tauri/tauri.conf.json");
    assert!(!text.contains("dangerous"), "a dangerous* flag is committed");
    assert!(!text.contains("http://"), "a plain-http endpoint is committed");
}
