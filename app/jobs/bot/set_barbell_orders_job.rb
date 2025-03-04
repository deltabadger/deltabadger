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
    # FIXME: If a manual retry is triggered, the retry_count increases in the database but not in sidekiq
    #        metadata. So far there's no clean way to access the sidekiq metadata, so we assume no manual
    #        retries are happening.
    #        Another option could be to use ActiveJob.retry_on exponential backoff, but that would ignore
    #        retries in the Sidekiq dashboard, and potentially ignore Sentry alerts.
    expected_delay = sidekiq_estimated_retry_delay(bot.retry_count)
    if expected_delay > 1.public_send(bot.interval)
      bot.notify_about_restart(errors: [e.message], delay: expected_delay)
    elsif expected_delay > 10.minutes # 5 failed attempts
      bot.notify_about_error(errors: [e.message])
    end
    bot.update!(status: :retrying, retry_count: bot.retry_count + 1)
    Bot::BroadcastStatusBarUpdateJob.perform_later(bot_id, 'next_set_barbell_orders_job_at.present?')
    raise e
  end

  private

  def sidekiq_estimated_retry_delay(retry_count)
    ((retry_count**4) + 15 + (rand(10) * (retry_count + 1))).seconds
  end
end
