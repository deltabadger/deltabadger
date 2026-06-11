class Bot::PriceDropLimitCheckJob < Bot::LimitCheckJobBase
  private

  def condition_result(bot)
    bot.get_price_drop_limit_condition_met?
  end

  def next_check_at(_bot)
    Time.now.utc.end_of_minute
  end
end
