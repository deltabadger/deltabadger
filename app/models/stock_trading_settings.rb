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

  # IBKR is hosted-only: its catalog is data-api served, so without the data API the
  # connect wizard is a dead end. Gate on the actual feed (env), not the selected
  # provider — a hosted DB later run self-hosted carries a stale 'deltabadger' row.
  #
  # The reason is MARKET DATA, not the catalog. IBKR does expose a list-all endpoint
  # (/trsrv/all-conids?exchange=&assetClass=), so a container holding a user's session
  # could in principle build its own catalog. What it cannot get is prices: IBKR
  # publishes no free feed and no history, and snapshot quotes need a paid per-exchange
  # subscription on that account. Unlike Alpaca, whose one key supplies catalog, prices
  # and candles alike (see Exchanges::Alpaca), IBKR is a broker only — every price
  # method on Exchanges::Ibkr is a stub deferring to the data-api feed.
  def self.ibkr_available?
    MarketDataSettings.deltabadger_available?
  end
end
