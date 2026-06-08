class Exchange::SyncAllTickersAndAssetsJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: 'sync_all_tickers_and_assets', on_conflict: :discard, duration: 4.hours

  def perform
    return unless MarketData.configured?

    # Stock venues (Alpaca, IBKR) sync via their own broker-specific path, not the
    # crypto market-data provider — so they're excluded from this loop.
    Exchange.available.where.not(type: Exchange::STOCK_TYPES).each_with_index do |exchange, i|
      Exchange::SyncTickersAndAssetsJob.set(wait: i * 1.minute).perform_later(exchange)
    end
  end
end
