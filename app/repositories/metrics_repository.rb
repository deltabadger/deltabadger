class MetricsRepository < BaseRepository
  METRICS_KEY = 'metrics'.freeze
  BOTS_IN_PROFIT_KEY = 'bots_in_profit'.freeze

  def initialize
    super
    @redis_client = Redis.new(url: ENV.fetch('REDIS_URL'))
  end

  def update_metrics
    telegram_metrics = FetchTelegramMetrics.new.call
    metrics = {
      liveBots: BotsRepository.new.count_with_status('working'),
      btcBought: convert_to_satoshis(TransactionsRepository.new.total_btc_bought),
      availableLegendaryBadgers: SubscriptionPlan.legendary.for_sale_count,
      takenLegendaryBadgersNftIds: Subscription.used_nft_ids,
      claimedLegendaryBadgersNftIds: Subscription.claimed_nft_ids,
      dca4yrProfitBtc: DcaProfitGetter.call('btc', 4.years.ago).data,
      dca4yrProfitEth: DcaProfitGetter.call('eth', 4.years.ago).data,
      dca4yrProfitXrp: DcaProfitGetter.call('xrp', 4.years.ago).data,
      dca4yrProfitSol: DcaProfitGetter.call('sol', 4.years.ago).data,
      dca4yrProfitBnb: DcaProfitGetter.call('bnb', 4.years.ago).data,
      dca4yrProfitDoge: DcaProfitGetter.call('doge', 4.years.ago).data,
      dca4yrProfitAda: DcaProfitGetter.call('ada', 4.years.ago).data,
      dca4yrProfitTrx: DcaProfitGetter.call('trx', 4.years.ago).data,
      dca4yrProfitAvax: DcaProfitGetter.call('avax', 4.years.ago).data,
      dca4yrProfitLink: DcaProfitGetter.call('link', 4.years.ago).data,
      dca4yrProfitShib: DcaProfitGetter.call('shib', 4.years.ago).data,
      dca4yrProfitTon: DcaProfitGetter.call('ton', 4.years.ago).data,
      # dca4yrProfitSp500: DcaProfitGetter.call('gspc', 4.years.ago).data,
      dca4yrProfitVoo: DcaProfitGetter.call('voo', 4.years.ago).data,
      # dca4yrProfitVti: DcaProfitGetter.call('vti', 4.years.ago).data,
      dca4yrProfitVt: DcaProfitGetter.call('vt', 4.years.ago).data,
      dca4yrProfitQqq: DcaProfitGetter.call('qqq', 4.years.ago).data,
      dca4yrProfitGld: DcaProfitGetter.call('gld', 4.years.ago).data
      # dca4yrProfitIta: DcaProfitGetter.call('ita', 4.years.ago).data
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
    Rails.logger.info('Fetching metrics data from Redis 0')
    metrics_response = @redis_client.get(METRICS_KEY)
    Rails.logger.info("Fetching metrics data from Redis 1 #{metrics_response.inspect}")
    bots_in_profit_response = @redis_client.get(BOTS_IN_PROFIT_KEY)
    Rails.logger.info("Fetching bots in profit data from Redis #{bots_in_profit_response.inspect}")
    data = {}
    Rails.logger.info("Fetching metrics data from Redis 2 #{data.inspect}")
    data.merge!(JSON.parse(metrics_response)) if metrics_response.present?
    Rails.logger.info("Fetching metrics data from Redis 3 #{data.inspect}")
    data.merge!(JSON.parse(bots_in_profit_response)) if bots_in_profit_response.present?
    Rails.logger.info("Fetching metrics data from Redis 4 #{data.inspect}")
    data
  end

  private

  def convert_to_satoshis(amount)
    (amount * 10**8).ceil
  end
end
