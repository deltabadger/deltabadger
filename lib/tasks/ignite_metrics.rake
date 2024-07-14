desc 'Sets the first fetch of the metrics and schedules the next one, starting the loop forever'
task ignite_metrics: [:environment] do
  UpdateMetricsWorker.perform_at(Time.now)
  UpdateBotsInProfitWorker.perform_at(Time.now)
end
