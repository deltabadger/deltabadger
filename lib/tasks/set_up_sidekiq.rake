desc 'Fill empty Sidekiq queue. Call it like "RAILS_ENV=production DRY_RUN=true rake set_up_sidekiq" to run in dry run mode'
task set_up_sidekiq: [:environment] do
  dry_run = ENV['DRY_RUN'] == 'true' if ENV['DRY_RUN']
  puts "Dry run mode: #{dry_run}"
  s = SetUpSidekiq.new
  s.fill_sidekiq_queue(dry_run: dry_run)
end
