class UpdateMetricsWorker
  include Sidekiq::Worker
  def perform
    MetricsRepository.new.update_metrics
    UpdateMetricsWorker.perform_at(Time.now + 6.hours)
  end
end
