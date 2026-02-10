class Exchange::SyncTickersAndAssetsJob < ApplicationJob
  queue_as :low_priority

  def perform(exchange)
    result = MarketData.sync_tickers!(exchange)
    raise result.errors.to_sentence if result.failure?
  end
end
