//! Plan B2: a Hyperliquid tokenized stock (an RWA spot listing, category Tokenized Stock) traded by Rails and by Rust over the
//! same bodies: script/rust/decisions.rb's grid-hyperliquid-tokenized, the Hyperliquid grid's scenarios on MU's pair @333.
//! Rails has no tokenized path on Hyperliquid, so every scenario is compared whole, with no listed divergence.
#![cfg(unix)] // shells out to bin/rails
mod common;

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_trade_a_hyperliquid_tokenized_stock_identically() {
    let outputs = common::venue_grid::run_venue_grid("grid-hyperliquid-tokenized", 20, &[]).await;
    // Pin what Rails does, so a silent pair of sides cannot pass.
    for (name, rails_out) in &outputs {
        let sent = rails_out["sent"].as_array().unwrap();
        // Nothing is sent under the 10-USDC floor, by a poll alone, or without a price: a book with no side has no mid, and a limit
        // bot prices from the mid (a missing ask alone does not stop it).
        let quiet = ["-below_minimum", "-no_mid", "-empty_book", "-sell_empty_book", "-poll_filled", "-poll_gone"].iter().any(|s| name.ends_with(s));
        assert_eq!(sent.len(), usize::from(!quiet), "{name}: {sent:?}");
        for order in sent { assert_eq!(order["a"], 10_142, "{name}: the asset is the pair's spotMeta index"); }
        let rows = rails_out["changes"]["transactions"].as_array().unwrap();
        let ids: Vec<&str> = rows.iter().filter_map(|r| r["after"]["external_id"].as_str()).collect();
        assert!(ids.iter().all(|id| id.starts_with("MU-")), "{name}: <base>-<oid>: {ids:?}");
    }
}
