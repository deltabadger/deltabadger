class FeesService
  FEES_KEY = 'fees_key'.freeze
  def update_fees
    redis_client = Redis.new(url: ENV.fetch('REDIS_AWS_URL'))
    table = redis_client.get(FEES_KEY).nil? ? [] : JSON.parse(redis_client.get(FEES_KEY))
    exchanges = Exchange.all
    exchanges.each do |exchange|
      exchange_market = ExchangeApi::Markets::Get.call(exchange.id)
      markets_current_fee = exchange_market.current_fee
      table[exchange.id] = markets_current_fee.to_s
    rescue StandardError
      table[exchange.id] = table[exchange.id].nil? ? ExchangeApi::Markets::BaseMarket.new.current_fee.to_s : table[exchange.id]
    end
    redis_client.set(FEES_KEY, table.to_json)
  end

  def current_fee(exchange_id)
    redis_client = Redis.new(url: ENV.fetch('REDIS_AWS_URL'))
    update_fees if redis_client.get(FEES_KEY).nil?
    JSON.parse(redis_client.get(FEES_KEY))[exchange_id]
  rescue StandardError
    ExchangeApi::Markets::BaseMarket.new.current_fee
  end
end
