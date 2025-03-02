class Bot::SetBarbellOrdersJob < BotJob
  def perform(bot_id)
    bot = Bot.find(bot_id)
    return unless bot.working? || bot.pending?

    bot.update!(status: :pending, last_set_barbell_orders_job_at_iso8601: Time.current.iso8601)
    result = bot.set_barbell_orders
    raise StandardError, "Failed to set barbell orders: #{result.errors}" unless result.success?

    Bot::SetBarbellOrdersJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot_id)
    bot.update!(status: :working)
  end
end
