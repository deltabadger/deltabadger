class Exchange::SyncTickersAndAssetsJob < ApplicationJob
  queue_as :default

  def perform(exchange)
    result = exchange.sync_tickers_and_assets_with_remote_data
    raise result.errors.to_sentence if result.failure?
  end
end
