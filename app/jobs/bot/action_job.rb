class Bot::ActionJob < BotJob
  DO_NOT_RETRY_ERRORS = [
    :insufficient_funds
  ].freeze

  retry_on Client::TransientNetworkError,
           wait: :polynomially_longer,
           attempts: 4 do |job, error|
    # Retries exhausted. Keep the bot in :retrying, log a visible
    # execution_failed entry, notify the user, and hand back to the bot's own
    # scheduler so a fresh attempt fires at the next interval.
    bot = job.arguments.first
    bot.update!(status: :retrying)
    bot.log_activity('execution_failed', level: :error,
                                         details: { error: error.message, ignorable: nil, transient_exhausted: true })
    bot.notify_about_error(errors: [bot.exchange.humanize_error(error.message)])
    Bot::ActionJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot)
    Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
  end

  def perform(bot)
    action_started_at = Time.current
    return unless bot.scheduled? || bot.retrying?
    raise "ActionJob for bot #{bot.id}: The bot already has an action job scheduled" if bot.next_action_job_at.present?

    bot.ensure_exchange_authenticated
    unless bot.exchange.market_open?
      Rails.logger.info("ActionJob for bot #{bot.id}: market closed, rescheduling to #{bot.exchange.next_market_open_at}")
      bot.update!(waiting_for_market_open: true)
      bot.log_activity('market_closed', details: { next_market_open_at: bot.exchange.next_market_open_at })
      Bot::ActionJob.set(wait_until: bot.exchange.next_market_open_at).perform_later(bot)
      Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
      return
    end

    bot.update!(last_action_job_at: Time.current, waiting_for_market_open: nil)
    result = bot.execute_action
    if result.failure?
      Rails.logger.error("ActionJob for bot #{bot.id} failed to execute action. Errors: #{result.errors.to_sentence}")
      raise result.errors.to_sentence
    end

    # The starting-time feature only affects the FIRST execution; flip it off
    # afterwards so the rule UI is free to be reconfigured for a future restart.
    bot.disable_starting_time! if bot.respond_to?(:disable_starting_time!) && bot.start_time_enabled?

    if result.data.present? && result.data[:break_reschedule]
      Rails.logger.info("ActionJob for bot #{bot.id} reschedule disabled.")
      bot.log_activity('reschedule_disabled')
    else
      # Skip the automatic broadcast - BroadcastAfterScheduledActionJob will handle it
      # after the next job is actually scheduled
      bot.instance_variable_set(:@skip_status_bar_broadcast, true)
      bot.update!(status: :scheduled)
      schedule_next_action_job(bot)
    end
  rescue Client::TransientNetworkError
    # Skip the noisy execution_failed / notify_retry path. Leaving the bot in
    # :retrying ensures the ActiveJob retry chain (and any post-exhaustion
    # reschedule) passes the line-8 guard on the next perform.
    bot.update!(status: :retrying)
    raise
  rescue StandardError => e
    Rails.logger.error("ActionJob for bot #{bot.id} failed to perform. Errors: #{e.message}")
    bot.update!(status: :retrying)
    category = ignorable_error_category(bot, e)
    # A failed order already records its own Transaction row; only log execution_failed
    # for failures that left no transaction (auth, market/API, unexpected errors).
    unless bot.transactions.failed.where('created_at >= ?', action_started_at).exists?
      bot.log_activity('execution_failed', level: :error, details: { error: e.message, ignorable: category })
    end
    if category
      notify_ignorable(bot, category, e)
      schedule_next_action_job(bot)
    else
      notify_retry(bot, e)
      Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
      raise e
    end
  end

  private

  def ignorable_error_category(bot, error)
    DO_NOT_RETRY_ERRORS.find do |category|
      messages = Array(bot.exchange.known_errors[category]).compact
      error.message.in?(messages)
    end
  end

  def notify_ignorable(bot, category, error)
    case category
    when :insufficient_funds
      bot.notify_end_of_funds
    else
      bot.notify_about_error(errors: humanized_errors(bot, error))
    end
  end

  def humanized_errors(bot, error)
    [bot.exchange.humanize_error(error.message)]
  end

  def estimated_retry_delay
    @estimated_retry_delay ||= begin
      next_retry_count = retry_count + 1
      ((next_retry_count**4) + 15 + (rand(10) * (next_retry_count + 1))).seconds
    end
  end

  def notify_retry(bot, error)
    if estimated_retry_delay > bot.effective_interval_duration
      bot.notify_about_error(errors: humanized_errors(bot, error))
    elsif estimated_retry_delay > 1.minute # 3 failed attempts
      bot.notify_about_error(errors: humanized_errors(bot, error))
    end
  end

  def schedule_next_action_job(bot)
    Bot::ActionJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot)
    Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
  end
end
