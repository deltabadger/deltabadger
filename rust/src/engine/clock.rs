//! Exchanges::Alpaca#market_open? and #next_market_open_at for one tick (exchanges/alpaca.rb:101-132, :644-662), read fresh
//! (the engine keeps no clock cache) and FAILING CLOSED (spec Amendment 2026-10-02b, a-e2's ruling). Rails reads any clock
//! failure that is not a transport raise or a rejected key as nil, and nil as open; here it places nothing and retries.
use crate::ruby::{time_as_json, to_sentence};
use crate::venue::{ClockAnswer, VenueError};
use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Debug, PartialEq)]
pub enum Gate {
    /// `is_open == true` and the session has not passed its own next_close.
    Open,
    /// Rails' closed path: park until `next_open`. `details` is next_market_open_at as Rails' JSON stores the parsed Time.
    Closed { next_open: DateTime<Utc>, details: String },
    /// Place nothing; fail the tick as transient (retry_on ×4, then the next checkpoint).
    Retry(String),
    /// Exchange#raise_on_invalid_key!: a StandardError naming the venue (execution_failed).
    InvalidKey(String),
}

pub fn gate(answer: Result<ClockAnswer, VenueError>, exchange_name: &str, now: DateTime<Utc>) -> Gate {
    let body = match answer {
        // Client.network_failure raises: Rails retries too (parity, `clock_timeout`).
        Err(VenueError::Transient(m) | VenueError::Ambiguous(m)) => return Gate::Retry(m),
        Err(VenueError::Rejected(e)) => return Gate::Retry(format!("market clock unavailable: {}", to_sentence(&e))),
        Ok(ClockAnswer::Failed { status, message }) => {
            // Exchange#invalid_key_error?: HTTP 401, or Exchanges::Alpaca::ERRORS[:invalid_key] ('unauthorized').
            if status == Some(401) || message.contains("unauthorized") { return Gate::InvalidKey(format!("{exchange_name} rejected the API key: {message}")); }
            // DIVERGENCE clock_5xx / clock_certificate: Rails reads nil as open.
            return Gate::Retry(format!("market clock unavailable: {message}"));
        }
        Ok(ClockAnswer::Body(b)) => b,
    };
    // DIVERGENCE clock_unreadable: Rails' JSON middleware fails, the Failure reads as nil, nil as open.
    let unreadable = || Gate::Retry("market clock unreadable".into());
    let Ok(v) = serde_json::from_str::<Value>(&body) else { return unreadable() };
    let (Some(is_open), Some(next_open), Some(next_close)) = (v["is_open"].as_bool(), v["next_open"].as_str(), v["next_close"].as_str()) else { return unreadable() };
    let (Ok(open_at), Ok(close_at)) = (DateTime::parse_from_rfc3339(next_open), DateTime::parse_from_rfc3339(next_close)) else { return unreadable() };
    if is_open {
        // DIVERGENCE clock_stale_body / clock_stale_cache: an open clock past its own close is stale; Rails places on it.
        if close_at.with_timezone(&Utc) <= now { return Gate::Retry(format!("market clock stale: open, but its next_close {next_close} has passed")); }
        return Gate::Open;
    }
    // DIVERGENCE clock_past_next_open: Rails re-enqueues at a past time and spins on its cached answer for up to a minute.
    if open_at.with_timezone(&Utc) <= now { return Gate::Retry(format!("market clock stale: closed, but its next_open {next_open} has passed")); }
    Gate::Closed { next_open: open_at.with_timezone(&Utc), details: time_as_json(next_open, &open_at) }
}
