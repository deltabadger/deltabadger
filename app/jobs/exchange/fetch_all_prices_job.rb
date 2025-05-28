class Exchange::FetchAllPricesJob < ApplicationJob
  queue_as :default

  def perform
    Exchange.available_for_new_bots.each do |exchange|
      Exchange::FetchPricesJob.perform_later(exchange)
    end
  end
end
