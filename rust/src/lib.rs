//! Deltabadger's Rust backend. It reads and writes the Rails app's SQLite files under the Rails
//! schema.
pub mod app_config;
pub mod codec;
pub mod crypto;
pub mod engine;
pub mod jobs;
pub mod enums;
pub mod figures;
pub mod lease;
pub mod mail;
pub mod parity;
pub mod ruby;
pub mod store;
pub mod supervisor;
pub mod sync;
pub mod venue;
pub mod web;
