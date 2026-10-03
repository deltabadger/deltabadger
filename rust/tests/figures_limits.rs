//! The hostile numbers of rust/examples/figures_limits.rs, in `cargo test`. The example is how they are run in the
//! release profile, where a panic aborts.
#[path = "../examples/figures_limits.rs"]
mod limits;

#[test]
fn hostile_numbers_are_refused_quickly_and_nothing_is_computed_from_them() {
    assert_eq!(limits::run(), Ok(87));
}
