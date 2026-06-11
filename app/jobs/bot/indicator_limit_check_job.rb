class Bot::IndicatorLimitCheckJob < Bot::LimitCheckJobBase
  private

  def condition_result(bot)
    bot.get_indicator_limit_condition_met?
  end

  def next_check_at(bot)
    Time.now.utc + Utilities::Time.seconds_to_current_candle_close(bot.indicator_limit_in_timeframe_duration)
  end
end
