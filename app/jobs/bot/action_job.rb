class Bot::ActionJob < BotJob
  DO_NOT_RETRY_ERRORS = [
    :insufficient_funds
  ].freeze

  def perform(bot) # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    return unless bot.scheduled? || bot.retrying?
    raise "ActionJob for bot #{bot.id}: The bot already has an action job scheduled" if bot.next_action_job_at.present?

    bot.update!(last_action_job_at: Time.current)
    result = bot.execute_action
    if result.failure?
      Rails.logger.error("ActionJob for bot #{bot.id} failed to execute action. Errors: #{result.errors.to_sentence}")
      raise result.errors.to_sentence.to_s
    end

    if result.data.present? && result.data[:break_reschedule]
      Rails.logger.info("ActionJob for bot #{bot.id} reschedule disabled.")
    else
      schedule_next_action_job(bot)
    end
  rescue StandardError => e
    Rails.logger.error("ActionJob for bot #{bot.id} failed to perform. Errors: #{e.message}")
    if ignorable_error?(bot, e)
      bot.notify_about_error(errors: [e.message])
      schedule_next_action_job(bot)
    else
      notify_retry(bot, e)
      bot.update!(status: :retrying)
      Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
      raise e
    end
  end

  private

  def ignorable_error?(bot, error)
    error.message.in?(bot.exchange.known_errors.select { |k, _| k.in?(DO_NOT_RETRY_ERRORS) }.values)
  end

  def sidekiq_estimated_retry_delay
    @sidekiq_estimated_retry_delay ||= begin
      next_retry_count = retry_count + 1
      ((next_retry_count**4) + 15 + (rand(10) * (next_retry_count + 1))).seconds
    end
  end

  def notify_retry(bot, error)
    if sidekiq_estimated_retry_delay > bot.interval_duration

      # the email message doesn't really make sense here, so we use notify_about_error instead
      # bot.notify_about_restart(errors: [error.message], delay: sidekiq_estimated_retry_delay)

      bot.notify_about_error(errors: [error.message])

    elsif sidekiq_estimated_retry_delay > 1.minute # 3 failed attempts
      bot.notify_about_error(errors: [error.message])
    end
  end

  def schedule_next_action_job(bot)
    Bot::ActionJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot)
    bot.update!(status: :scheduled)
    Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
  end
end
