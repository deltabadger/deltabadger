desc 'rake task to stop a stuck bot and unschedule its jobs. '\
     'Call it like this: rake stop_bot_and_unschedule\[27587\]'
task :stop_bot_and_unschedule, [:bot_id] => :environment do |_t, args|
  bot_id = args[:bot_id].to_i
  raise 'Bot ID must be provided!' if bot_id.blank? || bot_id.zero?

  bot = Bot.find(bot_id)
  raise 'Bot is not legacy!' unless bot.legacy?

  r = StopBot.call(bot.id)
  raise 'Failed to stop bot!' unless r.success?

  cancel_scheduled_jobs(bot)
  puts "Bot #{bot_id} stopped and jobs unscheduled"
end

def cancel_scheduled_jobs(bot)
  sidekiq_places = [
    Sidekiq::ScheduledSet.new,
    Sidekiq::Queue.new('default'),
    Sidekiq::RetrySet.new
  ]
  sidekiq_places.each do |place|
    place.each do |job|
      job.delete if job.queue == 'default' &&
                    job.display_class == 'Bot::BroadcastAfterScheduledActionJob' &&
                    job.display_args.first == [{ '_aj_globalid' => bot.to_global_id.to_s }].first
    end
  end
end
