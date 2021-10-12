desc 'Fill empty sidekiq queue'
task set_up_sidekiq: [:environment] do
  s = SetUpSidekiq.new
  s.fill_sidekiq_queue
end
