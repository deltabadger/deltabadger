mod common;
#[path="support/tracker_index_grid.rs"]
mod grid;
#[tokio::test(flavor="current_thread")]
async fn cold_holdings_match_rails_and_never_write() {
    let selected=std::env::var("D5_HOLDINGS_CASES").unwrap_or_default();
    let selected=selected.split(',').filter(|s|!s.is_empty()).collect::<Vec<_>>();
    grid::check_grid("script/rust/pages_tracker_holdings.rb","D5_HOLDINGS_GRID",131,&selected,|name|if name.starts_with("holdings_deferred_"){grid::Mode::Deferred}else{grid::Mode::Holdings}).await;
}
