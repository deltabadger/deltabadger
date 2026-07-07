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
end
