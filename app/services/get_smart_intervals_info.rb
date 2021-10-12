class GetSmartIntervalsInfo < BaseService
  def call(params, user)
    exchange_id = get_exchange_id(params)
    exchange_market = ExchangeApi::Markets::Get.call(exchange_id)
    symbol = exchange_market.symbol(params.fetch(:base, params['base']),
                                    params.fetch(:quote, params['quote']))

    minimum_order_params = exchange_market.minimum_order_parameters(symbol)
    return minimum_order_params unless minimum_order_params.success?

    price = params[:price].to_d
    smart_intervals = params[:force_smart_intervals]
    Result::Success.new(
      minimum: minimum_order_params.data[:minimum],
      minimum_limit: minimum_order_params.data.fetch(:minimum_limit, nil),
      minimumQuote: get_minimum_quote_price(minimum_order_params.data).round(2),
      side: minimum_order_params.data[:side],
      showSmartIntervalsInfo: smart_intervals?(user, price, minimum_order_params.data, smart_intervals)
    )
  rescue StandardError
    Result::Failure.new
  end

  def set_show_smart_intervals(user)
    user.update(show_smart_intervals_info: false)
  end

  private

  def smart_intervals?(user, price, minimum_order_params, force_smart_intervals)
    return false unless user.show_smart_intervals_info

    force_smart_intervals == 'true' || price <= get_minimum_quote_price(minimum_order_params)
  end

  def get_minimum_quote_price(minimum_order_params)
    return minimum_order_params[:minimum_quote].to_f if minimum_order_params[:side] == 'base'

    minimum_order_params[:minimum].to_f
  end

  def get_exchange_id(params)
    exchange_id = params[:exchange_id]
    return Exchange.where('LOWER(name) = ?', params[:exchange_name].downcase)[0].id if exchange_id.nil?

    exchange_id
  end
end
