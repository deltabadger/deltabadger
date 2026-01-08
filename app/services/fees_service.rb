class FeesService
  FEES_KEY = 'fees_key'.freeze

  def update_fees
    table = Rails.cache.read(FEES_KEY) || []
    exchanges = Exchange.all
    exchanges.each do |exchange|
      exchange_market = ExchangeApi::Markets::Get.call(exchange.id)
      markets_current_fee = exchange_market.current_fee
      table[exchange.id] = markets_current_fee.to_s
    rescue StandardError
      table[exchange.id] = table[exchange.id].nil? ? ExchangeApi::Markets::BaseMarket.new.current_fee.to_s : table[exchange.id]
    end
    Rails.cache.write(FEES_KEY, table)
  end

  def current_fee(exchange_id)
    update_fees if Rails.cache.read(FEES_KEY).nil?
    Rails.cache.read(FEES_KEY)[exchange_id]
  rescue StandardError
    ExchangeApi::Markets::BaseMarket.new.current_fee
  end
end
