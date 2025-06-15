desc 'Fill empty Sidekiq queue'
task set_up_sidekiq: [:environment] do
  s = SetUpSidekiq.new
  s.fill_sidekiq_queue(schedule: true) # false to test without scheduling new jobs
end
