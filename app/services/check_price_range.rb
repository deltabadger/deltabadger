class CheckPriceRange < BaseService
  # return true when price_range is not enabled or current price is in price range
  def call(bot)
    return Result::Success.new(valid: true) unless bot.price_range_enabled

    market = ExchangeApi::Markets::Get.new.call(bot.exchange_id)
    symbol = market.symbol(bot.base, bot.quote)
    current_rate = if bot.buyer?
                     market.current_ask_price(symbol)
                   else
                     market.current_bid_price(symbol)
                   end
    return current_rate unless current_rate.success?

    current_rate = current_rate_for_limit(current_rate, bot) if bot.limit?
    amount = CalculateTransactionQuantity.new.call(bot, current_rate.data)
    return amount unless amount.success?

    Result::Success.new(
      valid: price_within_range?(current_rate.data, bot),
      rate: current_rate.data,
      amount: amount.data
    )
  end

  private

  def price_within_range?(current_rate, bot)
    current_rate.between?(bot.price_range[0].to_f, bot.price_range[1].to_f)
  end

  def current_rate_for_limit(current_rate, bot)
    percentage = bot.buyer? ? -bot.percentage.to_f : bot.percentage.to_f
    Result::Success.new(current_rate.data * (1 + percentage / 100))
  end
end
