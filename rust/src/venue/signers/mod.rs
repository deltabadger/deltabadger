//! The request signers of the venues whose signing is their own (PLAN-VB §1.2, slice S): Gemini's payload, Coinbase's JWT
//! and Hyperliquid's EIP-712 L1 action. Pure functions of their inputs (key, clock, nonce), each pinned by known answers
//! recorded from honeymaker 0.12.5 / hyperliquid-rb 0.1.1 (`script/rust/record_venue_signing.rb`). Wiring them into the
//! transport's send-time signing is the venue slices' work (B1–B3).
pub mod coinbase;
pub mod gemini;
pub mod hyperliquid;

use std::sync::Mutex;

/// A process-wide nonce in ms, strictly increasing and never behind the clock: `max(now_ms, last + 1)` under a lock.
/// Honeymaker::Clients::Hyperliquid.next_nonce exactly; for Gemini it is SPEC-E §2.9's rule (honeymaker sends the plain
/// clock, so every nonce this gives is one Rails could have sent, and two calls in one ms never collide).
pub struct Nonce(Mutex<u64>);

impl Nonce {
    pub const fn new() -> Self { Self(Mutex::new(0)) }
    pub fn next_at(&self, now_ms: u64) -> u64 {
        let mut last = self.0.lock().unwrap_or_else(|p| p.into_inner());
        *last = now_ms.max(*last + 1);
        *last
    }
    pub fn next(&self) -> u64 { self.next_at(chrono::Utc::now().timestamp_millis().max(0) as u64) }
}

impl Default for Nonce { fn default() -> Self { Self::new() } }
