class CheckWithdrawalMinimums < BaseService
  def call(exchange_id, params)
    return params[:threshold] >= params[:minimum] unless minimum_defined_in_usd(exchange_id)

    market = ExchangeApi::Markets::Get.call(exchange_id)
    symbol = market.symbol(params[:currency], 'USD')
    ask = market.current_ask_price(symbol)
    return true unless ask.success?

    ask.data * params[:threshold] >= params[:minimum]
  end

  private

  def minimum_defined_in_usd(exchange_id)
    exchange = Exchange.find(exchange_id)
    %w[ftx ftx.us].include?(exchange.name.downcase)
  end
end
