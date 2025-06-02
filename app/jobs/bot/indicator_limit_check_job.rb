class Bot::IndicatorLimitCheckJob < ApplicationJob
  queue_as :default

  def perform(bot)
    return unless bot.waiting?

    if bot.indicator_limit_condition_met?
      bot.update!(started_at: Time.current)
      Bot::ActionJob.perform_later(bot)
    else
      next_check_at = Time.now.utc.end_of_day + 30.seconds # give 30s of buffer to avoid checking before the previous candle was closed
      Bot::IndicatorLimitCheckJob.set(wait_until: next_check_at).perform_later(bot)
    end
  end
end
