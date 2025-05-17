class Bot::PriceLimitCheckJob < ApplicationJob
  queue_as :default

  def perform(bot)
    return unless bot.waiting?

    if bot.price_limit_condition_met?
      bot.update!(started_at: Time.current)
      Bot::ActionJob.perform_later(bot)
    else
      # Add 10 seconds to the next check to let Exchange::FetchAllPricesJob cron job feed the cache
      next_check_at = Time.current + Utilities::Time.seconds_to_end_of_minute + 10.seconds
      Bot::PriceLimitCheckJob.set(wait_until: next_check_at).perform_later(bot)
    end
  end
end
