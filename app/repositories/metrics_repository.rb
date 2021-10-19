class MetricsRepository < BaseRepository
  METRICS_KEY = 'metrics'.freeze
  def update_metrics
    telegram_metrics = FetchTelegramMetrics.new.call
    redis_client = Redis.new(url: ENV.fetch('REDIS_AWS_URL'))
    output_params = {
      liveBots: BotsRepository.new.count_with_status('working'),
      btcBought: convert_to_satoshis(TransactionsRepository.new.total_btc_bought),
      btcBoughtDayAgo: convert_to_satoshis(TransactionsRepository.new.total_btc_bought_day_ago)
    }.merge(telegram_metrics)
    redis_client.set(METRICS_KEY, output_params.to_json)
  end

  def metrics_data
    redis_client = Redis.new(url: ENV.fetch('REDIS_AWS_URL'))
    redis_response = redis_client.get(METRICS_KEY)
    JSON.parse(redis_response)
  end

  def profitable_bots_data
    total_profitable = 0
    transactions_totals = Bot.select('bot_id, sum (amount * rate) as total_cost, sum (amount) as total_amount, exchange_id, settings')
                             .joins(:transactions).group(:bot_id, :exchange_id, :settings).where("settings->>'type' = 'buy'")
    actual_length = transactions_totals.length
    prices_dictionary = {}
    markets_dictionary = {}
    transactions_totals.each_with_index do |total, index|
      dictionary_key = dictionary_key(total['settings']['base'], total['settings']['quote'])
      if prices_dictionary[dictionary_key].nil?
        if markets_dictionary[total['exchange_id']].nil?
          market = ExchangeApi::Markets::Get.new.call(total['exchange_id'])
          markets_dictionary[total['exchange_id']] = market
        else
          market = markets_dictionary[total['exchange_id']]
        end
        symbol = market.symbol(total['settings']['base'], total['settings']['quote'])
        current_price = market.current_price(symbol)
        prices_dictionary[dictionary_key] = current_price
      else
        current_price = prices_dictionary[dictionary_key]
      end
      if current_price.data.nil? || total['total_amount'].nil? || !current_price.success?
        actual_length -= 1
        next
      end
      current_value = total['total_amount'] * current_price.data
      total_profitable += 1 if current_value > total['total_cost']
      print "Done: #{(index / transactions_totals.length.to_f * 100).ceil(2)}% \r"
    end
    puts "Profitable bots: #{(total_profitable / actual_length.to_f * 100).ceil(2)}%"
  end

  private

  def dictionary_key(base,quote)
    "#{base}-#{quote}"
  end

  def convert_to_satoshis(amount)
    (amount * 10**8).ceil
  end
end
