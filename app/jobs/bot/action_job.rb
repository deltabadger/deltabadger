class Bot::ActionJob < BotJob
  DO_NOT_RETRY_ERRORS = [
    :insufficient_funds
  ].freeze

  # Retries exhausted. Keep the bot in :retrying and reschedule a fresh attempt at the next
  # interval. Transient (network / -1021 timestamp) exhaustion is self-recovering: log a calm,
  # de-emphasized (:info) entry and DON'T email "your bot failed". Everything else (incl.
  # rate-limit exhaustion) stays red (:error) + notifies. NOTE: activity rows aren't currently
  # color-coded by level, so the user-visible lever here is the suppressed email; :info keeps it
  # gray, not yellow, if level styling is added later.
  EXHAUSTION_HANDLER = lambda do |job, error, exhausted_detail|
    bot = job.arguments.first
    next unless Bot::ActionJob.transition_working_bot!(bot, 'retrying')

    if exhausted_detail[:transient_exhausted]
      bot.log_activity('execution_retrying', level: :info,
                                             details: { error: error.message }.merge(exhausted_detail))
    else
      bot.log_activity('execution_failed', level: :error,
                                           details: { error: error.message, ignorable: nil }.merge(exhausted_detail))
      bot.notify_about_error(errors: [bot.exchange.humanize_error(error.message)])
    end
    Bot::ActionJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot)
    Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
  end

  retry_on Client::TransientNetworkError,
           wait: :polynomially_longer,
           attempts: 4 do |job, error|
    EXHAUSTION_HANDLER.call(job, error, transient_exhausted: true)
  end

  # Rate limits retry on their own longer, escalating wait (BotJob::RATE_LIMIT_WAIT) so we
  # don't keep re-tripping the exchange's decaying counter.
  retry_on Client::RateLimitedError,
           wait: BotJob::RATE_LIMIT_WAIT,
           attempts: 4 do |job, error|
    EXHAUSTION_HANDLER.call(job, error, rate_limited_exhausted: true)
  end

  # A stop or delete from another process (user click, admin stock deactivation sweep) can land
  # while this job is mid-flight on a stale bot instance; Stop's cancel can't reach a Claimed
  # (running) execution, and a check-then-update! would leave a resurrection window. The
  # conditional UPDATE closes it: the flip only happens if no stop/delete won the race, and the
  # caller branches on the outcome. Skipped callbacks are covered on every call site: the status
  # bar is (re)broadcast by BroadcastAfterScheduledActionJob or explicitly, and the
  # button/columns-lock UI does not differ between working statuses.
  def self.transition_working_bot!(bot, status)
    updated = Bot.where(id: bot.id).where.not(status: %w[stopped deleted])
                 .update_all(status: status, updated_at: Time.current) == 1
    # Sync the attribute without reload — reload would drop memoized associations; only the
    # status column changed, and no later save runs on these paths.
    bot.status = status if updated
    updated
  end

  def perform(bot)
    action_started_at = Time.current
    return unless bot.scheduled? || bot.retrying?
    raise "ActionJob for bot #{bot.id}: The bot already has an action job scheduled" if bot.next_action_job_at.present?

    # An IBKR key registered but not yet activated by IBKR (24h–2wk). Reschedule WITHOUT touching
    # the exchange — a pending key must never reach a live IBKR call. Ibkr::CheckActivationJob flips
    # the key to :correct on activation, and the next run proceeds.
    if bot.api_key&.pending_activation?
      Rails.logger.info("ActionJob for bot #{bot.id}: api_key pending IBKR activation, rescheduling")
      schedule_next_action_job(bot)
      return
    end

    bot.ensure_exchange_authenticated
    unless bot.exchange.market_open?
      Rails.logger.info("ActionJob for bot #{bot.id}: market closed, rescheduling to #{bot.exchange.next_market_open_at}")
      bot.update!(waiting_for_market_open: true)
      bot.log_activity('market_closed', details: { next_market_open_at: bot.exchange.next_market_open_at })
      Bot::ActionJob.set(wait_until: bot.exchange.next_market_open_at).perform_later(bot)
      Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
      return
    end

    # Market confirmed open: clear any stale market-closed flag immediately, bypassing validation
    # (Fix C). Otherwise a separate problem — e.g. a temporarily-unavailable ticker that makes the
    # success-path update! below raise — would leave the flag stuck and the UI showing "market
    # closed" for a non-market issue.
    clear_stale_market_closed_flag(bot)

    bot.update!(last_action_job_at: Time.current, waiting_for_market_open: nil)
    result = bot.execute_action
    if result.failure?
      Rails.logger.error("ActionJob for bot #{bot.id} failed to execute action. Errors: #{result.errors.to_sentence}")
      # A -1021/timestamp rejection on placement is a definitive pre-trade rejection (the order was
      # never placed — see Exchange#placement_transient_error?). It is self-recovering: reschedule a
      # fresh attempt at the next interval (clean — no orphan), log a calm gray entry, and send NO
      # alarm email. Everything else stays the existing red path (raise → rescue StandardError).
      if bot.exchange.placement_transient_error?(result.errors)
        return unless self.class.transition_working_bot!(bot, 'retrying')

        bot.log_activity('execution_retrying', level: :info,
                                               details: { error: result.errors.to_sentence, placement_transient: true })
        schedule_next_action_job(bot)
        return
      end
      raise result.errors.to_sentence
    end

    # The starting-time feature only affects the FIRST execution; flip it off
    # afterwards so the rule UI is free to be reconfigured for a future restart.
    bot.disable_starting_time! if bot.respond_to?(:disable_starting_time!) && bot.start_time_enabled?

    if result.data.present? && result.data[:break_reschedule]
      Rails.logger.info("ActionJob for bot #{bot.id} reschedule disabled.")
      bot.log_activity('reschedule_disabled')
    elsif self.class.transition_working_bot!(bot, 'scheduled')
      # No broadcast here on purpose — BroadcastAfterScheduledActionJob handles it after the
      # next job is actually scheduled.
      schedule_next_action_job(bot)
    else
      Rails.logger.info("ActionJob for bot #{bot.id}: bot was stopped mid-execution, leaving it stopped")
    end
  rescue Client::TransientNetworkError, Client::RateLimitedError
    # Skip the noisy execution_failed / notify_retry path. Leaving the bot in
    # :retrying ensures the ActiveJob retry chain (and any post-exhaustion
    # reschedule) passes the line-8 guard on the next perform.
    bot.broadcast_status_bar_update if self.class.transition_working_bot!(bot, 'retrying')
    raise
  rescue StandardError => e
    Rails.logger.error("ActionJob for bot #{bot.id} failed to perform. Errors: #{e.message}")
    unless self.class.transition_working_bot!(bot, 'retrying')
      Rails.logger.info("ActionJob for bot #{bot.id}: bot was stopped mid-execution, skipping retry handling")
      return
    end

    bot.broadcast_status_bar_update
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

  def clear_stale_market_closed_flag(bot)
    return unless bot.waiting_for_market_open

    bot.update_columns(transient_data: bot.transient_data.merge('waiting_for_market_open' => nil))
  end

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
