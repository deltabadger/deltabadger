class UpdateBotsInProfitJob < ApplicationJob
  queue_as :default

  def perform
    MetricsRepository.new.update_bots_in_profit
  end
end
