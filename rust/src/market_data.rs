//! MarketDataSettings (app/models/market_data_settings.rb) and AppConfig's market-data readers (app_config.rb:134-150,
//! :257-289), as one pure rule. Every reader of the install's market-data configuration (web::bots::market_data,
//! jobs::data_api::config, engine::provider) reads its four rows its own way and asks this function what they mean.
use crate::jobs::data_api::Config;

/// The `app_configs` rows: None when the row does not exist, Some (possibly blank) when it does. A NULL value is Some("").
#[derive(Clone, Debug, Default)]
pub struct Rows { pub provider: Option<String>, pub url: Option<String>, pub token: Option<String>, pub coingecko_key: Option<String> }

/// `MARKET_DATA_URL`, `MARKET_DATA_TOKEN`, `COINGECKO_API_KEY` as the process started with them.
#[derive(Clone, Debug, Default)]
pub struct Env { pub url: Option<String>, pub token: Option<String>, pub coingecko_key: Option<String> }

impl Env {
    pub fn read(env: &dyn Fn(&str) -> Option<String>) -> Self {
        Self { url: env("MARKET_DATA_URL"), token: env("MARKET_DATA_TOKEN"), coingecko_key: env("COINGECKO_API_KEY") }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Provider {
    /// The deltabadger provider: its URL and token as AppConfig.market_data_url / _token read them (either may be blank).
    DataApi(Config),
    /// The coingecko provider, and AppConfig.coingecko_api_key: None when blank (every CoinGecko call then fails
    /// "CoinGecko API key not configured").
    CoinGecko { key: Option<String> },
    /// No provider, or a provider row this build does not know.
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub provider: Provider,
    /// MarketDataSettings.configured?
    pub configured: bool,
}

fn present(v: &Option<String>) -> bool { v.as_deref().is_some_and(|s| !s.trim().is_empty()) }

/// `record ? record.value : ENV.fetch(var, '')`.
fn row_or_env(row: &Option<String>, env: &Option<String>) -> String { row.clone().or_else(|| env.clone()).unwrap_or_default() }

pub fn decide(rows: &Rows, env: &Env) -> Settings {
    let available = present(&env.url); // deltabadger_available?
    // current_provider: deltabadger when available, else the row (AppConfig.get: the row's value).
    let current = if available { Some("deltabadger".to_string()) } else { rows.provider.clone() };
    let url = row_or_env(&rows.url, &env.url);
    let token = row_or_env(&rows.token, &env.token);
    let configured = if !available && current.as_deref() == Some("deltabadger") {
        !url.trim().is_empty() && !token.trim().is_empty()
    } else {
        present(&current)
    };
    let provider = match current.as_deref() {
        Some("deltabadger") => Provider::DataApi(Config { url, token }),
        Some("coingecko") => {
            let key = row_or_env(&rows.coingecko_key, &env.coingecko_key);
            Provider::CoinGecko { key: (!key.trim().is_empty()).then_some(key) }
        }
        _ => Provider::None,
    };
    Settings { provider, configured }
}

/// The four rows through `app_config` (decrypted; a NULL value reads as ""), then `decide`.
pub fn read(c: &rusqlite::Connection, cipher: &crate::crypto::Cipher, env: &Env) -> Result<Settings, String> {
    let row = |key: &str| -> Result<Option<String>, String> {
        Ok(if crate::app_config::exists(c, key)? { Some(crate::app_config::get(c, cipher, key)?.unwrap_or_default()) } else { None })
    };
    let rows = Rows { provider: row("market_data_provider")?, url: row("market_data_url")?, token: row("market_data_token")?, coingecko_key: row("coingecko_api_key")? };
    Ok(decide(&rows, env))
}

impl Settings {
    pub fn deltabadger(&self) -> bool { matches!(self.provider, Provider::DataApi(_)) }
    pub fn coingecko(&self) -> bool { matches!(self.provider, Provider::CoinGecko { .. }) }
    /// The CoinGecko key when the provider is coingecko and a key is set.
    pub fn coingecko_key(&self) -> Option<&str> {
        match &self.provider { Provider::CoinGecko { key } => key.as_deref(), _ => None }
    }
    /// The data-api configuration with both values present, as the engine's index fence reads it.
    pub fn data_api_complete(&self) -> Option<&Config> {
        match &self.provider { Provider::DataApi(c) if !c.url.trim().is_empty() && !c.token.trim().is_empty() => Some(c), _ => None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(v: &str) -> Option<String> { Some(v.into()) }

    #[test]
    fn env_url_wins_over_the_row() {
        let out = decide(&Rows { provider: s("coingecko"), coingecko_key: s("k"), ..Default::default() }, &Env { url: s("http://data-api:3000"), token: s("t"), ..Default::default() });
        assert_eq!(out.provider, Provider::DataApi(Config { url: "http://data-api:3000".into(), token: "t".into() }));
        assert!(out.configured);
    }

    #[test]
    fn a_db_selected_deltabadger_needs_url_and_token() {
        let rows = Rows { provider: s("deltabadger"), url: s("https://x"), ..Default::default() };
        assert!(!decide(&rows, &Env::default()).configured);
        assert!(decide(&rows, &Env { token: s("t"), ..Default::default() }).configured, "the token falls back to the environment");
        let blank = Rows { token: s(""), ..rows };
        assert!(!decide(&blank, &Env { token: s("t"), ..Default::default() }).configured, "a blank row is not overridden by the environment");
    }

    #[test]
    fn coingecko_is_configured_even_without_a_key() {
        let out = decide(&Rows { provider: s("coingecko"), coingecko_key: s(""), ..Default::default() }, &Env { coingecko_key: s("env"), ..Default::default() });
        assert!(out.configured);
        assert_eq!(out.coingecko_key(), None, "a blank row wins over COINGECKO_API_KEY");
        let env = decide(&Rows { provider: s("coingecko"), ..Default::default() }, &Env { coingecko_key: s("env"), ..Default::default() });
        assert_eq!(env.coingecko_key(), Some("env"));
    }

    #[test]
    fn no_provider_and_unknown_provider() {
        assert_eq!(decide(&Rows::default(), &Env::default()), Settings { provider: Provider::None, configured: false });
        let other = decide(&Rows { provider: s("other"), ..Default::default() }, &Env::default());
        assert_eq!(other, Settings { provider: Provider::None, configured: true });
    }
}
