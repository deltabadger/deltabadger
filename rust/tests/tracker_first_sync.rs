//! Populated first-sync pages: exact Rails bytes except the documented unknown-value cell.
mod common;
#[path="support/tracker_index_grid.rs"]
mod grid;
#[tokio::test(flavor="current_thread")]
async fn populated_first_sync_pages_match_rails_and_never_write() {
    grid::check_grid("script/rust/pages_tracker_first_sync.rb","D5_FIRST_GRID",105,&[], |name|if name.starts_with("first_deferred_"){grid::Mode::Deferred}else if name.starts_with("first_bad_"){grid::Mode::Exact}else{grid::Mode::FirstSync}).await;
}
