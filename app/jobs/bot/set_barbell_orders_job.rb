class Bot::SetBarbellOrdersJob < BotJob
  def perform(bot)
    return unless bot.scheduled? || bot.retrying?
    raise StandardError, "bot #{bot.id} already has a job scheduled" if bot.next_action_job_at.present?

    bot.notify_if_funds_are_low
    bot.update!(status: :executing, last_action_job_at: Time.current)
    total_orders_amount_in_quote = [
      bot.pending_quote_amount,
      bot.quote_amount_available_before_limit_reached
    ].min
    result = bot.set_barbell_orders(
      total_orders_amount_in_quote: total_orders_amount_in_quote,
      update_missed_quote_amount: true
    )
    raise StandardError, result.errors.to_sentence unless result.success?

    bot.update!(status: :scheduled) if bot.executing?
    Bot::SetBarbellOrdersJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot)
    Bot::BroadcastStatusBarUpdateAfterScheduledOrderJob.perform_later(bot)
  rescue StandardError => e
    Rails.logger.error("Error setting barbell orders for bot #{bot.id}: #{e.message}")
    if sidekiq_estimated_retry_delay > 1.public_send(bot.interval)
      bot.notify_about_restart(errors: [e.message], delay: sidekiq_estimated_retry_delay)
    elsif sidekiq_estimated_retry_delay > 1.minute # 3 failed attempts
      bot.notify_about_error(errors: [e.message])
    end
    bot.update!(status: :retrying)
    Bot::BroadcastStatusBarUpdateAfterScheduledOrderJob.perform_later(bot)
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
