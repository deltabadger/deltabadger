class Bot::SetBarbellOrdersJob < BotJob
  def perform(bot_id)
    bot = Bot.find(bot_id)
    return unless bot.working? || bot.retrying?

    bot.update!(status: :pending, last_set_barbell_orders_job_at_iso8601: Time.current.iso8601)
    result = bot.set_barbell_orders
    raise StandardError, result.errors.to_sentence unless result.success?

    bot.update!(status: :working, retry_count: 0)
    Bot::SetBarbellOrdersJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot_id)
    Bot::BroadcastStatusBarUpdateJob.perform_later(bot_id, 'next_set_barbell_orders_job_at.present?')
  rescue StandardError => e
    # FIXME: We could use ActiveJob retry_on exponential backoff and builtin executions instead of
    #        middleware-injected retry_count from Sidekiq. However, this would ignore retries being
    #        placed in the retries section in the Sidekiq dashboard, and potentially ignore Sentry alerts.
    if sidekiq_estimated_retry_delay > 1.public_send(bot.interval)
      bot.notify_about_restart(errors: [e.message], delay: expected_delay)
    elsif sidekiq_estimated_retry_delay > 10.minutes # 5 failed attempts
      bot.notify_about_error(errors: [e.message])
    end
    bot.update!(status: :retrying)
    Bot::BroadcastStatusBarUpdateJob.perform_later(bot_id, 'next_set_barbell_orders_job_at.present?')
    raise e
  end

  private

  def sidekiq_estimated_retry_delay
    next_retry_count = retry_count + 1
    ((next_retry_count**4) + 15 + (rand(10) * (next_retry_count + 1))).seconds
  end
end
