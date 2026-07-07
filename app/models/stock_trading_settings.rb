# frozen_string_literal: true

# Single answer-point for "is stock trading available on this container?".
# Hosted (platform market data): the data API provides the stock catalog, so
# stocks are always on. Self-hosted: the admin's Alpaca credential is only a
# catalog-sync bootstrap — stocks are active whenever a synced catalog exists,
# credential or not. Per-user trading credentials are NOT this class's
# concern — they live in each user's ApiKeys.
class StockTradingSettings
  def self.active?
    deltabadger? || Ticker.available.where(exchange: Exchange.stock_venues).exists?
  end

  def self.deltabadger?
    MarketDataSettings.deltabadger?
  end

  # IBKR is hosted-only: its catalog is data-api served. There is no self-hosted
  # source (IBKR has no list-all-instruments endpoint and no free market data),
  # so without the data API the connect wizard is a dead end. Gate on the actual
  # feed (env), not the selected provider — a hosted DB later run self-hosted
  # carries a stale 'deltabadger' provider row.
  def self.ibkr_available?
    MarketDataSettings.deltabadger_available?
  end
end
