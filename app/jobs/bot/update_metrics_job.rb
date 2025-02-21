class Bot::UpdateMetricsJob < BotJob
  queue_as :default

  # FIXME: ideally calling this job should kill any running Bot::UpdateMetricsJob
  # for the same bot, but by now we are only cancelling other enqueued jobs.

  def perform(bot_id)
    cancel_other_jobs(bot_id)
    bot = Bot.find(bot_id)
    bot.metrics(recalculate: true)
  end

  private

  def cancel_other_jobs(bot_id)
    sidekiq_places = [
      Sidekiq::RetrySet.new,
      Sidekiq::ScheduledSet.new,
      Sidekiq::Queue.new(queue_name)
    ]
    sidekiq_places.each do |place|
      place.each do |job|
        job.delete if job.queue == queue_name &&
                      job.display_class == 'Bot::UpdateMetricsJob' &&
                      job.display_args == [bot_id]
      end
    end
  end
end
