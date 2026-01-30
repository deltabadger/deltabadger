class Exchange::SyncAllTickersAndAssetsJob < ApplicationJob
  queue_as :low_priority

  def perform
    return unless AppConfig.coingecko_configured?

    Exchange.available.each_with_index do |exchange, i|
      Exchange::SyncTickersAndAssetsJob.set(wait: i * 1.minute).perform_later(exchange)
    end
  end
end
