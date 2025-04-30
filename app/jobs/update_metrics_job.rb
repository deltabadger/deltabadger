class UpdateMetricsJob < ApplicationJob
  queue_as :default

  def perform
    metrics = Metrics.new
    metrics.update_metrics
  end
end
