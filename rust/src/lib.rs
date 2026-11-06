//! Deltabadger's Rust backend. It reads and writes the Rails app's SQLite files under the Rails
//! schema.
/// The release's version, src-tauri/Cargo.toml's (build.rs, D-ENGINE-VERSION).
pub const VERSION: &str = env!("DELTABADGER_VERSION");

pub mod app_config;
pub mod asset_taxonomy;
pub mod bundle;
pub mod codec;
pub mod crypto;
pub mod engine;
pub mod identity;
pub mod jobs;
pub mod enums;
pub mod figures;
pub mod lease;
pub mod logger;
pub mod market_data;
pub mod mail;
pub mod parity;
pub mod password_input;
pub mod rotation;
pub mod ruby;
pub mod stop;
pub mod store;
pub mod supervisor;
pub mod sync;
pub mod tax;
pub mod tracker;
pub mod venue;
pub mod web;
