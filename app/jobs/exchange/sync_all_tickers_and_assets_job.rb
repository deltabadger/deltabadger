class Exchange::SyncAllTickersAndAssetsJob < ApplicationJob
  queue_as :low_priority

  def perform
    Exchange.available_for_new_bots.each_with_index do |exchange, i|
      Exchange::SyncTickersAndAssetsJob.set(wait: i * 1.minute).perform_later(exchange)
    end
  end
end
