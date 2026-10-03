//! What one figure may cost to compute. Ruby's BigDecimal has no limit, and Rails' accounting grows its numbers:
//! a history of sales the venue did not price doubles a number's digits with every sale, which takes Rails four
//! times as long each time (minutes after fifteen such sales, days after twenty); a long history of priced sales
//! keeps, for every point of its chart, a cost of tens of thousands of digits.
//!
//! Two meters, both in limbs of nine digits, both deterministic: a history costs the same on every machine and
//! in every profile, so a test can hold a walk to them.
//! - **Steps**: every decimal operation is charged the limbs it is about to touch, before it touches them, and every
//!   comparison of two instants counts one. This is time.
//! - **Held**: the limbs of the numbers made in the scope and still alive. This is memory: the chart keeps a
//!   number per holding per point.
//!
//! A figure that passes either is not computed (`NumError::OverBudget`). The meters are kept per thread. Each
//! figure this library computes runs in a scope of its own (`within`); a caller that wants several figures under
//! one pair of limits wraps them in one scope. Outside any scope an operation is held to the limits by itself, so
//! no arithmetic anywhere is unbounded.
use super::num::NumError;
use std::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits { pub steps: u64, pub held: u64 }

/// What a scope used: the steps it took, and the most limbs it held at one time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Used { pub steps: u64, pub held: u64 }

/// The limits of one figure, chosen from measurement (script/rust/figures.rb's `priced_pairs` and
/// `unpriced_rebalances` are the histories; rust/examples/figures_limits.rs prints what each costs here).
///
/// Steps: one billion. All figures of 100,000 buys take 20 million; 700 priced sale/buy pairs take 23 million.
/// The walk of 4,000 priced pairs takes 285 million (Rails accounting: about one second). Eleven sales with no
/// reported proceeds, each bought back, take 805 million for all figures (Rails accounting: 1.5 seconds);
/// the twelfth exceeds this budget. Rails then takes four times as long with every pair.
///
/// Held: 64 million limbs, which is 256 megabytes of digits. The 4,000 priced sales and buys hold 57 million: a
/// cost of up to 128,000 digits at each of 8,000 points.
pub const FIGURE: Limits = Limits { steps: 1_000_000_000, held: 64_000_000 };

thread_local! {
    static STEPS: Cell<u64> = const { Cell::new(0) };
    /// The limbs alive in this thread's numbers, in and out of scopes.
    static ALIVE: Cell<u64> = const { Cell::new(0) };
    /// The open scope: its limits, the limbs that were alive when it opened, and the most it has held since.
    static SCOPE: Cell<Option<(Limits, u64, u64)>> = const { Cell::new(None) };
}

fn held_now(alive_at_start: u64) -> u64 { ALIVE.try_with(Cell::get).unwrap_or(0).saturating_sub(alive_at_start) }

struct Restore { steps: u64, scope: Option<(Limits, u64, u64)> }

impl Drop for Restore {
    fn drop(&mut self) {
        // What an inner scope took, its outer scope took too.
        STEPS.set(if self.scope.is_some() { self.steps.saturating_add(STEPS.get()) } else { 0 });
        SCOPE.set(self.scope);
    }
}

/// Runs `work` in a scope of its own with these limits, and says what it used.
pub fn scope<T>(limits: Limits, work: impl FnOnce() -> T) -> (T, Used) {
    let restore = Restore { steps: STEPS.get(), scope: SCOPE.get() };
    STEPS.set(0);
    SCOPE.set(Some((limits, ALIVE.get(), 0)));
    let out = work();
    let used = Used { steps: STEPS.get(), held: SCOPE.get().map_or(0, |(_, _, peak)| peak) };
    drop(restore);
    (out, used)
}

/// Runs `work` in the scope that is open, or in one of its own with the limits of a figure.
pub fn within<T>(work: impl FnOnce() -> T) -> T {
    if SCOPE.get().is_some() { work() } else { scope(FIGURE, work).0 }
}

/// Before an operation that will take `steps` and make a number of `limbs`: refused when the scope cannot afford
/// either. Nothing has been allocated yet.
pub fn charge(steps: u64, limbs: u64) -> Result<(), NumError> {
    let Some((limits, start, _)) = SCOPE.get() else {
        return if steps > FIGURE.steps || limbs > FIGURE.held { Err(NumError::OverBudget) } else { Ok(()) };
    };
    let taken = STEPS.get().saturating_add(steps);
    if taken > limits.steps || held_now(start).saturating_add(limbs) > limits.held { return Err(NumError::OverBudget); }
    STEPS.set(taken);
    Ok(())
}

/// Steps that cannot be refused where they happen (a comparison): counted, and `check` refuses at the next turn
/// of the loop they are in.
pub fn step(steps: u64) {
    if SCOPE.get().is_some() { STEPS.set(STEPS.get().saturating_add(steps)); }
}

pub fn check() -> Result<(), NumError> { charge(0, 0) }

/// A number of `limbs` has been made (`dec` calls this, once per number) ...
pub(super) fn hold(limbs: u64) {
    // `try_with`: a number may be made or dropped while its thread is ending, when the meters are gone already.
    let _ = ALIVE.try_with(|alive| alive.set(alive.get().saturating_add(limbs)));
    let _ = SCOPE.try_with(|scope| if let Some((limits, start, peak)) = scope.get() { scope.set(Some((limits, start, peak.max(held_now(start))))); });
}

/// ... and is no more.
pub(super) fn release(limbs: u64) { let _ = ALIVE.try_with(|alive| alive.set(alive.get().saturating_sub(limbs))); }
