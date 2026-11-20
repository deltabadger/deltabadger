-- #878 (PLAN-878, D-ALERT-OPT-OUTS): the bot alert emails a user switched off in Settings, a row per alert. No row is on,
-- so every user starts with all three. Rust-only: 2.x never reads it, and sends every alert while it owns the install.
CREATE TABLE rust_email_alert_opt_outs (
  user_id INTEGER NOT NULL,
  alert TEXT NOT NULL CHECK(alert IN ('low_funds','bot_errors','bot_stopped')),
  PRIMARY KEY (user_id, alert)
);
