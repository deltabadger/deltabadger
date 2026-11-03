//! B9 proof (temporary, removed before merge): a test file platform_excluded.txt doesn't name runs on every leg.
#[test]
fn b9_probe_runs_on_every_platform_leg() {
    eprintln!("b9 probe ran on {}", std::env::consts::OS);
}
