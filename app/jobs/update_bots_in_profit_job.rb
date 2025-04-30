class UpdateBotsInProfitJob < ApplicationJob
  queue_as :default

  def perform
    metrics = Metrics.new
    metrics.update_bots_in_profit
  end
end
