class Exchange::SyncAllTickersAndAssetsJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: -> { name }, on_conflict: :discard, duration: 4.hours

  def perform
    return unless MarketData.configured?

    Exchange.available.where.not(type: 'Exchanges::Alpaca').each_with_index do |exchange, i|
      Exchange::SyncTickersAndAssetsJob.set(wait: i * 1.minute).perform_later(exchange)
    end
  end
end
