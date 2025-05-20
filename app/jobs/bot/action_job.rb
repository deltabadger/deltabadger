class Bot::ActionJob < BotJob
  def perform(bot)
    return unless bot.scheduled? || bot.retrying?
    raise StandardError, "bot #{bot.id} already has an action job scheduled" if bot.next_action_job_at.present?

    bot.update!(last_action_job_at: Time.current)
    result = bot.execute_action
    raise StandardError, result.errors.to_sentence unless result.success?

    unless result.data[:break_reschedule]
      bot.update!(status: :scheduled)
      Bot::ActionJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot)
      Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
    end
  rescue StandardError => e
    Rails.logger.error("Error executing action job for bot #{bot.id}: #{e.message}")
    notify_retry(bot, e)
    bot.update!(status: :retrying)
    Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
    raise e
  end

  private

  def sidekiq_estimated_retry_delay
    @sidekiq_estimated_retry_delay ||= begin
      next_retry_count = retry_count + 1
      ((next_retry_count**4) + 15 + (rand(10) * (next_retry_count + 1))).seconds
    end
  end

  def notify_retry(bot, error)
    if sidekiq_estimated_retry_delay > 1.public_send(bot.interval)
      bot.notify_about_restart(errors: [error.message], delay: sidekiq_estimated_retry_delay)
    elsif sidekiq_estimated_retry_delay > 1.minute # 3 failed attempts
      bot.notify_about_error(errors: [error.message])
    end
  end
end
