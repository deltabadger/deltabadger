//! B9 proof (temporary, removed before merge): a test file platform_excluded.txt doesn't name runs on every leg.
#[test]
fn b9_probe_runs_on_every_platform_leg() {
    eprintln!("b9 probe ran on {}", std::env::consts::OS);
}

#[test]
fn b9_probe_fails_on_macos_only() {
    #[cfg(target_os = "macos")]
    panic!("B9 proof: a macOS-only failure turns the macos-arm64 leg red");
}
