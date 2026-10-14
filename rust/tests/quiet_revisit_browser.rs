//! A revisit of a bot page in a real browser (script/rust/quiet_revisit_check.mjs): the compiled bundle
//! both apps serve, on fixture pages, in headless Chrome. What it checks lives in the browser alone: the
//! morph that answers Turbo's preview, the chart updating in place, the stream subscriptions it keeps,
//! and the tooltip it must not orphan. Needs Chrome and bun; the full suite's runner has both.

#[test]
fn a_bot_page_revisit_keeps_what_the_preview_drew() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let out = std::process::Command::new("bun")
        .arg(format!("{root}/script/rust/quiet_revisit_check.mjs"))
        .output()
        .expect("bun runs");
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}
