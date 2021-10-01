class UpdateMetricsWorker
  include Sidekiq::Worker
  def perform
    MetricsRepository.new.update_metrics
    UpdateMetricsWorker.perform_at(Time.now + 1.minute)
  end
end