class ClearHostedAlpacaAppConfig < ActiveRecord::Migration[8.1]
  # On hosted containers (platform market data present) the container-global
  # Alpaca credential was only ever written as a side effect of per-user
  # connects and has no consumer: the catalog comes from the data API
  # (SyncAlpacaAssetsJob no-ops) and trading always uses per-user ApiKeys.
  # Drop the leftovers so one user's trading credential doesn't linger
  # container-global. Self-hosted (no MARKET_DATA_URL) keeps the credential —
  # it drives the weekly catalog sync.
  def up
    return if ENV['MARKET_DATA_URL'].blank?

    execute "DELETE FROM app_configs WHERE key IN ('alpaca_api_key', 'alpaca_api_secret', 'alpaca_mode')"
  end

  def down; end
end
