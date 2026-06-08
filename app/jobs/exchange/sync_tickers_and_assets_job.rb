class Exchange::SyncTickersAndAssetsJob < ApplicationJob
  queue_as :low_priority

  def perform(exchange)
    # Stock venues (Alpaca, IBKR) don't sync tickers through the crypto market-data
    # provider — the data-api has no catalog for them and rejects the request
    # ({"error":"Invalid exchange: ibkr"}). They sync via their own broker-specific path.
    return if exchange.stock_venue?

    result = MarketData.sync_tickers!(exchange)
    raise result.errors.to_sentence if result.failure?
  end
end
