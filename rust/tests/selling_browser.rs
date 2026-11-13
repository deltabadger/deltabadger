//! The bot page's Sell links while a sale runs, in a real browser (script/rust/selling_check.mjs): the compiled bundle's
//! bot--selling controller on a fixture page. Inert links, the spinner, a click and Enter that open nothing, a panel
//! published mid-sale, the unblock, and the "no longer held" flash run from inside the modal frame. Needs Chrome and bun;
//! the full suite's runner has both. D-LIQUIDATION-ONE-LINE.

#[test]
fn sell_links_are_inert_while_a_sale_runs_and_live_again_after() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let out = std::process::Command::new("bun")
        .arg(format!("{root}/script/rust/selling_check.mjs"))
        .output()
        .expect("bun runs");
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}
