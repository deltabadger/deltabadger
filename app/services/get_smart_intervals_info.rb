class GetSmartIntervalsInfo < BaseService
  def call(params)
    exchange_id = params[:exchange_id]
    exchange_market = ExchangeApi::Markets::Get.call(exchange_id)
    symbol = exchange_market.symbol(params[:base], params[:quote])

    minimum_order_params = exchange_market.minimum_order_parameters(symbol)
    return minimum_order_params['minimum'] unless minimum_order_params['minimum'].success?

    price = params[:price]
    smart_intervals = params[:force_smart_intervals]
    minimum_order_params.merge(
      will_trigger_smart_intervals: smart_intervals?(price, minimum_order_params, smart_intervals)
    )
  end

  private

  def smart_intervals?(price, minimum_order_params, force_smart_intervals)
    force_smart_intervals || price <= minimum_order_params['minimum']
  end
end
