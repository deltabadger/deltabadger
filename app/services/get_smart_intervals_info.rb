class GetSmartIntervalsInfo < BaseService
  def call(params)
    exchange_id = params[:exchange_id]
    exchange_market = ExchangeApi::Markets::Get.call(exchange_id)
    symbol = exchange_market.symbol(params[:base], params[:quote])

    minimum_order_params = exchange_market.minimum_order_parameters(symbol)
    return minimum_order_params unless minimum_order_params.success?

    price = params[:price].to_d
    smart_intervals = params[:force_smart_intervals]
    Result::Success.new(
      minimum: minimum_order_params.data[:minimum],
      minimum_quote: get_minimum_quote_price(minimum_order_params.data).round(2),
      side: minimum_order_params.data[:side],
      will_trigger_smart_intervals: smart_intervals?(price, minimum_order_params.data, smart_intervals)
    )
  end

  private

  def smart_intervals?(price, minimum_order_params, force_smart_intervals)
    force_smart_intervals == 'true' || price <= get_minimum_quote_price(minimum_order_params)
  end

  def get_minimum_quote_price(minimum_order_params)
    return minimum_order_params[:minimum_quote].to_f if minimum_order_params[:side] == 'base'

    minimum_order_params[:minimum].to_f
  end
end
