class MetricsRepository < BaseRepository
  METRICS_KEY = 'metrics'.freeze
  BOTS_IN_PROFIT_KEY = 'bots_in_profit'.freeze
  EARLY_BIRD_DISCOUNT_INITIAL_VALUE = (ENV.fetch('EARLY_BIRD_DISCOUNT_INITIAL_VALUE').to_i || 0).freeze

  def initialize
    super
    @redis_client = Redis.new(url: ENV.fetch('REDIS_AWS_URL'))
  end

  def update_metrics
    telegram_metrics = FetchTelegramMetrics.new.call
    metrics = {
      liveBots: BotsRepository.new.count_with_status('working'),
      btcBought: convert_to_satoshis(TransactionsRepository.new.total_btc_bought),
      availableLegendaryBadgers: EARLY_BIRD_DISCOUNT_INITIAL_VALUE - SubscriptionsRepository.new.all_current_count('legendary_badger'),
      takenLegendaryBadgerNumbers: SubscriptionsRepository.new.model.used_sequence_numbers
    }.merge(telegram_metrics)
    @redis_client.set(METRICS_KEY, metrics.to_json)
  end

  def update_bots_in_profit
    Scenic.database.refresh_materialized_view(:bots_total_amounts, concurrently: true, cascade: false)
    profitable_bots_data = ProfitableBotsRepository.new.profitable_bots_data
    bots_in_profit = {
      profitBotsTillNow: profitable_bots_data[0],
      profitBots12MonthsAgo: profitable_bots_data[1]
    }
    @redis_client.set(BOTS_IN_PROFIT_KEY, bots_in_profit.to_json)
    FeesService.new.update_fees
  end

  def metrics_data
    metrics_response = @redis_client.get(METRICS_KEY)
    bots_in_profit_response = @redis_client.get(BOTS_IN_PROFIT_KEY)
    JSON.parse(metrics_response).merge(JSON.parse(bots_in_profit_response))
  end

  private

  def convert_to_satoshis(amount)
    (amount * 10**8).ceil
  end
end
