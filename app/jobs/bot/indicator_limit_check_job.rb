class Bot::IndicatorLimitCheckJob < ApplicationJob
  queue_as :default

  def perform(bot)
    return unless bot.waiting?

    result = bot.get_indicator_limit_condition_met?
    if result.failure?
      Rails.logger.warn("IndicatorLimitCheckJob for bot #{bot.id} failed: #{result.errors.to_sentence}. Retrying in 1 minute.")
      Bot::IndicatorLimitCheckJob.set(wait_until: 1.minute.from_now).perform_later(bot)
      return
    end

    if result.data
      bot.update!(status: :scheduled)
      Bot::ActionJob.perform_later(bot)
    else
      next_check_at = Time.now.utc + Utilities::Time.seconds_to_current_candle_close(bot.indicator_limit_in_timeframe_duration)
      Bot::IndicatorLimitCheckJob.set(wait_until: next_check_at).perform_later(bot)
    end
  end
end
