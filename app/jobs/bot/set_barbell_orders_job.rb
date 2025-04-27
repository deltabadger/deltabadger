class Bot::SetBarbellOrdersJob < BotJob
  def perform(bot)
    return unless bot.working? || bot.retrying?

    bot.update!(status: :pending, last_action_job_at_iso8601: Time.current.iso8601)
    result = bot.set_barbell_orders
    raise StandardError, result.errors.to_sentence unless result.success?

    bot.update!(status: :working) if bot.pending?
    Bot::SetBarbellOrdersJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot)
    #  Schedule the broadcast status bar update to make sure sidekiq has time to schedule the job
    Bot::BroadcastStatusBarUpdateJob.set(wait: 0.25.seconds).perform_later(bot)
  rescue StandardError => e
    if sidekiq_estimated_retry_delay > 1.public_send(bot.interval)
      bot.notify_about_restart(errors: [e.message], delay: sidekiq_estimated_retry_delay)
    elsif sidekiq_estimated_retry_delay > 10.minutes # 5 failed attempts
      bot.notify_about_error(errors: [e.message])
    end
    bot.update!(status: :retrying)
    #  Schedule the broadcast status bar update to make sure sidekiq has time to schedule the job
    Bot::BroadcastStatusBarUpdateJob.set(wait: 0.25.seconds).perform_later(bot)
    raise e
  end

  private

  def sidekiq_estimated_retry_delay
    @sidekiq_estimated_retry_delay ||= begin
      next_retry_count = retry_count + 1
      ((next_retry_count**4) + 15 + (rand(10) * (next_retry_count + 1))).seconds
    end
  end
end
