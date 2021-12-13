class UpdateBotsInProfitWorker
  include Sidekiq::Worker
  def perform
    MetricsRepository.new.update_bots_in_profit
    UpdateBotsInProfitWorker.perform_at(Time.now + 24.hours)
  end
end
