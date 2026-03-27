class FeesService
  FEES_KEY = 'fees_key'.freeze
  DEFAULT_FEE = 0.1

  def update_fees
    table = Rails.cache.read(FEES_KEY) || []
    exchanges = Exchange.all
    exchanges.each do |exchange|
      exchange_market = ExchangeMarket.for(exchange.id)
      markets_current_fee = exchange_market.current_fee
      table[exchange.id] = markets_current_fee.to_s
    rescue StandardError
      table[exchange.id] = table[exchange.id].nil? ? DEFAULT_FEE.to_s : table[exchange.id]
    end
    Rails.cache.write(FEES_KEY, table)
  end

  def current_fee(exchange_id)
    update_fees if Rails.cache.read(FEES_KEY).nil?
    Rails.cache.read(FEES_KEY)[exchange_id]
  rescue StandardError
    DEFAULT_FEE
  end
end
