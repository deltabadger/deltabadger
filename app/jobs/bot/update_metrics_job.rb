class Bot::UpdateMetricsJob < BotJob
  queue_as :default

  # FIXME: ideally calling this job should kill any running Bot::UpdateMetricsJob
  # for the same bot, but by now we are only cancelling other enqueued jobs.

  def perform(bot)
    cancel_other_jobs(bot)
    bot.metrics(recalculate: true)
  end

  private

  def cancel_other_jobs(bot)
    sidekiq_places = [
      Sidekiq::RetrySet.new,
      Sidekiq::ScheduledSet.new,
      Sidekiq::Queue.new(queue_name)
    ]
    sidekiq_places.each do |place|
      place.each do |job|
        job.delete if job.queue == queue_name &&
                      job.display_class == 'Bot::UpdateMetricsJob' &&
                      job.display_args == [{ '_aj_globalid' => bot.to_global_id.to_s }]
      end
    end
  end
end
