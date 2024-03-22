class MetricsRepository < BaseRepository
  METRICS_KEY = 'metrics'.freeze
  BOTS_IN_PROFIT_KEY = 'bots_in_profit'.freeze

  def initialize
    super
    @redis_client = Redis.new(url: ENV.fetch('REDIS_AWS_URL'))
  end

  def update_metrics
    telegram_metrics = FetchTelegramMetrics.new.call
    legendary_badger_stats = PaymentsManager::LegendaryBadgerStatsCalculator.call.data
    metrics = {
      liveBots: BotsRepository.new.count_with_status('working'),
      btcBought: convert_to_satoshis(TransactionsRepository.new.total_btc_bought),
      availableLegendaryBadgers: legendary_badger_stats[:for_sale_legendary_badger_count],
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
    data = {}
    data.merge!(JSON.parse(metrics_response)) if metrics_response.present?
    data.merge!(JSON.parse(bots_in_profit_response)) if bots_in_profit_response.present?
  end

  private

  def convert_to_satoshis(amount)
    (amount * 10**8).ceil
  end
end
