class CheckExceededFrequency < BaseService
  def call(exchange_id, type, price, base, quote, currency_of_minimum, interval, force_smart_intervals, smart_intervals_value)
    return false unless force_smart_intervals

    frequency_limit = ENV['ORDERS_FREQUENCY_LIMIT']
    market = ExchangeApi::Markets::Get.new.call(exchange_id)
    symbol = market.symbol(base, quote)
    market_price = market.current_price(symbol).data
    price = price.to_f
    smart_intervals_value = smart_intervals_value.to_f
    smart_intervals_value /= market_price if currency_of_minimum == quote
    price /= market_price if %w[market_buy limit_buy].include?(type)

    frequency = price / smart_intervals_value
    frequency /= duration_in_hours(interval)
    frequency.to_f > frequency_limit.to_f
  end

  private

  def duration_in_hours(interval)
    {
      'day': 24,
      'week': 24 * 7,
      'month': 24 * 30
    }.fetch(interval, 1)
  end
end
