class Exchange::SyncAllTickersAndAssetsJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: 'sync_all_tickers_and_assets', on_conflict: :discard, duration: 4.hours

  # Instances sharing one market-data server all fire this from the same cron second, so without an
  # offset they request the same exchange's tickers at once. One random offset per tick spreads them
  # across the minute and keeps this instance's own exchanges a minute apart, as before. Only on the
  # deltabadger provider: a CoinGecko instance shares no server with its neighbours.
  JITTER_WINDOW = 1.minute

  def perform
    return unless MarketData.configured?

    offset = MarketDataSettings.deltabadger? ? rand(JITTER_WINDOW.to_i).seconds : 0.seconds

    # Stock venues (Alpaca, IBKR) sync via their own broker-specific path, not the
    # crypto market-data provider — so they're excluded from this loop.
    Exchange.available.where.not(type: Exchange::STOCK_TYPES).each_with_index do |exchange, i|
      Exchange::SyncTickersAndAssetsJob.set(wait: offset + (i * 1.minute)).perform_later(exchange)
    end
  end
end
