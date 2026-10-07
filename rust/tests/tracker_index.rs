//! Exact entry-page bytes and every primary/queue row, including round-1 regressions.
//! The complete tracker page grid remains a separate acceptance gate.
mod common;
use common::web::{self, Browser, TestClock};
#[path="support/tracker_index_grid.rs"]
mod grid;
use grid::snapshot;

#[tokio::test(flavor="current_thread")]
async fn early_index_pages_match_rails_and_every_get_keeps_all_rows() { check_grid(&[]).await; }

#[tokio::test(flavor="current_thread")]
async fn repeated_scalar_dates_use_the_last_value() {
    check_grid(&["index_from_last_blank","index_from_last_date","index_to_last_blank","index_to_last_date"]).await;
}

#[tokio::test(flavor="current_thread")]
async fn turbo_frame_skips_unreadable_navbar_but_full_layout_keeps_rails_error() {
    check_grid(&["index_unreadable_color_frame","index_unreadable_color_full"]).await;
}

async fn check_grid(selected:&[&str]) {
    grid::check_grid("script/rust/pages_tracker_index.rb","D5_INDEX_GRID",82,selected,|name| {
        if name=="index_broker_unconfigured" { return grid::Mode::FirstSync; }
        if ["index_populated","index_filtered_empty","index_bad_date","index_pending","index_from_last_date","index_to_last_date"].contains(&name) {grid::Mode::Deferred}else{grid::Mode::Exact}
    }).await;
}

#[tokio::test(flavor="current_thread")]
async fn index_requires_authentication_and_deferred_links_refuse_without_writes() {
    let(dir,opened,_)=common::install_alpaca();
    let app=web::app(dir.path(),web::SECRET,TestClock::at("2026-09-10T12:00:30Z"));
    let before=snapshot(&opened.primary);let mut browser=Browser::default();
    let answer=browser.get(&app,"/de/tracker").await;
    assert_eq!(answer.status,302);assert_eq!(answer.header("location"),Some("/de/login"));
    for path in ["/tracker/pick_exchange/new","/tracker/connect_market_data","/tracker/import/new","/tracker/export","/tracker/download_tax_report"] {
        let answer=browser.get(&app,path).await;assert_eq!(answer.status,501,"{path}");
        assert!(answer.body.contains(path));
    }
    assert_eq!(snapshot(&opened.primary),before);
}
