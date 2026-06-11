class Bot::MovingAverageLimitCheckJob < Bot::LimitCheckJobBase
  private

  def condition_result(bot)
    bot.get_moving_average_limit_condition_met?
  end

  def next_check_at(bot)
    Time.now.utc + Utilities::Time.seconds_to_current_candle_close(bot.moving_average_limit_in_timeframe_duration)
  end
end
