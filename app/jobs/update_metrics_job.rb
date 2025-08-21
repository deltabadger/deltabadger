class UpdateMetricsJob < ApplicationJob
  queue_as :low_priority

  def perform
    metrics = Metrics.new
    metrics.update_metrics
  end
end
