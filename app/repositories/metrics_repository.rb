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

  private

  def convert_to_satoshis(amount)
    (amount * 10**8).ceil
  end
end
