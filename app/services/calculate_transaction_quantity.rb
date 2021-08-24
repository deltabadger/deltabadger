class CalculateTransactionQuantity < BaseService
  BASE = 'base'.freeze

  # return true when price_range is not enabled or current price is in price range
  def call(bot, current_rate)
    volume = (bot.price.to_f / current_rate)
    return Result::Success.new(volume) unless bot.force_smart_intervals

    market = ExchangeApi::Markets::Get.new.call(bot.exchange_id)
    symbol = market.symbol(bot.base, bot.quote)
    minimum_params = market.minimum_order_parameters(symbol)
    return minimum_params unless minimum_params.success?

    min_value = if base?(minimum_params.data, bot)
                  bot.smart_intervals_value
                else
                  (bot.smart_intervals_value.to_f / current_rate)
                end

    Result::Success.new(min_value.to_f)
  end

  private

  def base?(minimum_params, bot)
    (bot.type == 'limit' && limit_defined_in_base?(minimum_params)) ||
      minimum_params[:side] == BASE
  end

  def limit_defined_in_base?(minimum_params)
    minimum_params[:minimum_limit].present?
  end
end
