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

// A build must never start without a real updater key and the secrets that sign with it — an
// unsigned or unverifiable artifact is worse than no artifact. The gate skips rather than fails,
// because the same tag also builds the Docker image and an unconfigured desktop pipeline must not
// redden an ordinary release, so what needs pinning is that the build cannot run without it.
#[test]
fn desktop_releases_cannot_build_without_a_verified_updater_key_and_signing_secrets() {
    let workflow = read(".github/workflows/desktop-release.yml");
    // The preflight job: its lines up to the next job (a line of two spaces and a name).
    let preflight: String = workflow
        .lines()
        .skip_while(|l| *l != "  preflight:")
        .skip(1)
        .take_while(|l| !(l.starts_with("  ") && l[2..].starts_with(|c: char| c.is_ascii_lowercase())))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!preflight.is_empty(), "desktop release workflow is missing the preflight job");
    assert!(preflight.contains("REPLACE_WITH_TAURI_UPDATER_PUBLIC_KEY"));
    assert!(preflight.contains("tauri signer generate"));
    // Signing is mandatory (createUpdaterArtifacts is v1Compatible) and Tauri reads these with
    // var_os, so an unset secret is an empty string and takes the import path rather than skipping.
    for secret in ["TAURI_SIGNING_PRIVATE_KEY", "APPLE_CERTIFICATE", "APPLE_ID", "APPLE_TEAM_ID"] {
        assert!(preflight.contains(secret), "preflight does not require {secret}");
    }
    assert!(workflow.contains("\n  build:\n    needs: preflight\n    if: needs.preflight.outputs.ready == 'true'\n"));
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
