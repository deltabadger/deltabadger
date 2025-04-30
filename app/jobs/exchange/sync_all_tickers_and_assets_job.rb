class Exchange::SyncAllTickersAndAssetsJob < ApplicationJob
  queue_as :default

  def perform
    Exchange.available_for_barbell_bots.each do |exchange|
      Exchange::SyncTickersAndAssetsJob.perform_later(exchange)
    end
  end
end
