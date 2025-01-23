class UpdateMetricsJob < ApplicationJob
  queue_as :default

  def perform
    MetricsRepository.new.update_metrics
  end
end
