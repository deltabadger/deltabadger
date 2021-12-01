class CheckExceededFrequency < BaseService
  def call(params)
    frequency_limit = ENV['ORDERS_FREQUENCY_LIMIT']
    unless params[:forceSmartIntervals] == 'true'
      return {
        limit_exceeded: false,
        new_intervals_value: params[:smartIntervalsValue].to_f,
        frequency_limit: frequency_limit
      }
    end

    market = get_market(params[:exchange_id])
    symbol = get_symbol(market, params[:base], params[:quote])
    market_price = get_market_price(market, symbol)
    defined_in_quote = defined_in_quote?(params[:currency_of_minimum], params[:quote])
    smart_intervals_in_base = get_smart_intervals_in_base(params[:smartIntervalsValue].to_f, market_price, defined_in_quote)
    price_in_base = get_price_in_base(params[:price].to_f, market_price, params[:type])
    frequency = get_frequency(price_in_base, smart_intervals_in_base, params[:interval])
    limit_exceeded = limit_exceeded?(frequency, frequency_limit)
    new_intervals_value = if limit_exceeded
                            get_new_intervals_value(
                              params[:price].to_f, frequency_limit, params[:interval],
                              market_price, defined_in_quote, market, symbol, params[:type]
                            )
                          else
                            params[:smartIntervalsValue].to_f
                          end
    {
      limit_exceeded: limit_exceeded,
      new_intervals_value: new_intervals_value,
      frequency_limit: frequency_limit
    }
  end

  private

  def defined_in_quote?(currency_of_minimum, quote)
    currency_of_minimum == quote
  end

  def get_market(exchange_id)
    ExchangeApi::Markets::Get.new.call(exchange_id)
  end

  def get_symbol(market, base, quote)
    market.symbol(base, quote)
  end

  def get_market_price(market, symbol)
    market.current_price(symbol).data
  end

  def get_price_in_base(price, market_price, type)
    return price / market_price if %w[market_buy limit_buy].include?(type)

    price
  end

  def get_smart_intervals_in_base(smart_intervals_value, market_price, defined_in_quote)
    return smart_intervals_value / market_price if defined_in_quote

    smart_intervals_value
  end

  def get_frequency(price_in_base, smart_intervals_in_base, interval)
    frequency = price_in_base / smart_intervals_in_base
    frequency /= duration_in_hours(interval)
    frequency
  end

  def limit_exceeded?(frequency, frequency_limit)
    frequency.to_f > frequency_limit.to_f
  end

  def get_new_intervals_value(price, frequency_limit, interval, market_price, defined_in_quote, market, symbol, type)
    new_intervals_value = price / (frequency_limit.to_f * duration_in_hours(interval))
    if defined_in_quote
      new_intervals_value *= market_price unless %w[market_buy limit_buy].include?(type)
      new_intervals_value = new_intervals_value.ceil(market.quote_decimals(symbol).data)
    else
      new_intervals_value /= market_price if %w[market_buy limit_buy].include?(type)
      new_intervals_value = new_intervals_value.ceil(market.base_decimals(symbol).data)
    end
    new_intervals_value
  end

  def duration_in_hours(interval)
    {
      'hour': 1,
      'day': 24,
      'week': 24 * 7,
      'month': 24 * 30
    }[interval.to_sym]
  end
end
