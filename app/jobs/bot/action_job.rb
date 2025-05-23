class Bot::ActionJob < BotJob
  def perform(bot)
    start_time = Time.current
    Rails.logger.info("Action job for bot #{bot.id} started at #{start_time}. Bot status: #{bot.status}." + (bot.next_action_job_at.present? ? "Next action job at: #{bot.next_action_job_at}." : ''))
    return unless bot.scheduled? || bot.retrying?
    raise StandardError, "bot #{bot.id} already has an action job scheduled" if bot.next_action_job_at.present?

    bot.update!(last_action_job_at: start_time)
    result = bot.execute_action
    raise StandardError, result.errors.to_sentence unless result.success?

    if result.data.present? && result.data[:break_reschedule]
      Rails.logger.info("Action job for bot #{bot.id} reschedule disabled.")
    else
      bot.update!(status: :scheduled)
      next_interval_checkpoint_at = bot.next_interval_checkpoint_at
      Rails.logger.info("Action job for bot #{bot.id} rescheduled at #{next_interval_checkpoint_at}.")
      Bot::ActionJob.set(wait_until: next_interval_checkpoint_at).perform_later(bot)
      Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
    end
    Rails.logger.info("Action job for bot #{bot.id} finished at #{Time.current}. Took #{Time.current - start_time} seconds.")
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
    if sidekiq_estimated_retry_delay > bot.interval_duration
      bot.notify_about_restart(errors: [error.message], delay: sidekiq_estimated_retry_delay)
    elsif sidekiq_estimated_retry_delay > 1.minute # 3 failed attempts
      bot.notify_about_error(errors: [error.message])
    end
  end
end
