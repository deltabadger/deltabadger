class MetricsRepository < BaseRepository
  METRICS_KEY = 'metrics'.freeze
  def update_metrics
    telegram_metrics = FetchTelegramMetrics.new.call
    Scenic.database.refresh_materialized_view(:bots_total_amounts, concurrently: true, cascade: false)
    redis_client = Redis.new(url: ENV.fetch('REDIS_AWS_URL'))
    profitable_bots_data = ProfitableBotsRepository.new.profitable_bots_data
    output_params = {
      liveBots: BotsRepository.new.count_with_status('working'),
      btcBought: convert_to_satoshis(TransactionsRepository.new.total_btc_bought),
      btcBoughtDayAgo: convert_to_satoshis(TransactionsRepository.new.total_btc_bought_day_ago),
      profitBotsTillNow: profitable_bots_data[3],
      profitBots3MothsAgo: profitable_bots_data[2],
      profitBots6MothsAgo: profitable_bots_data[1],
      profitBots12MothsAgo: profitable_bots_data[0]
    }.merge(telegram_metrics)
    redis_client.set(METRICS_KEY, output_params.to_json)
    FeesService.new.update_fees
  end

  def measure_time_of_execution
    t1 = Time.now
    update_metrics
    execution_time = Time.now - t1
    puts "Excecution time: #{execution_time}"
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
