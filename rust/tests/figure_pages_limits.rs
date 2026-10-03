#[path="../examples/figure_pages_limits.rs"]
mod limits;
#[test]
fn page_arithmetic_and_serialization_share_the_release_budget() {
    limits::run().unwrap();
}
