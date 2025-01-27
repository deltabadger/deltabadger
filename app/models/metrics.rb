class Metrics
  METRICS_KEY = 'metrics'.freeze
  BOTS_IN_PROFIT_KEY = 'bots_in_profit'.freeze

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
    redis_client.set(METRICS_KEY, metrics.to_json)
  end

  def update_bots_in_profit
    Scenic.database.refresh_materialized_view(:bots_total_amounts, concurrently: true, cascade: false)
    profitable_bots = profitable_bots_data
    bots_in_profit = {
      profitBotsTillNow: profitable_bots[0],
      profitBots12MonthsAgo: profitable_bots[1]
    }
    redis_client.set(BOTS_IN_PROFIT_KEY, bots_in_profit.to_json)
    FeesService.new.update_fees
  end

  def metrics_data
    Rails.logger.info('Fetching metrics data from Redis 0')
    metrics_response = redis_client.get(METRICS_KEY)
    Rails.logger.info("Fetching metrics data from Redis 1 #{metrics_response.inspect}")
    bots_in_profit_response = redis_client.get(BOTS_IN_PROFIT_KEY)
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

  def redis_client
    @redis_client ||= Redis.new(url: ENV.fetch('REDIS_URL'))
  end

  def convert_to_satoshis(amount)
    (amount * 10**8).ceil
  end

  def profitable_bots_data
    bot_totals = ActiveRecord::Base.connection.execute('select * from bots_total_amounts')
    filtered_bots_map = bot_totals.to_a.map { |bot| profitable_or_nil(bot) }.compact
    bots_in_periods = split_bots_by_period(filtered_bots_map)
    total_amount_of_bots = bots_in_periods.map(&:length)
    total_amount_of_bots_in_profit = bots_in_periods.map { |bot| bot.count { |b| b[0] } }
    bots_in_periods.each_index.map do |index|
      percentage(total_amount_of_bots_in_profit[0..index].sum, total_amount_of_bots[0..index].sum)
    end.reverse
  end

  def prices_dictionary
    @prices_dictionary ||= {}
  end

  def markets_dictionary
    @markets_dictionary ||= {}
  end

  def percentage(amount, total)
    (amount / total.to_f * 100).ceil(2)
  end

  def profitable_or_nil(bot)
    settings = JSON.parse(bot['settings'])
    current_price = current_price(bot['exchange_id'], settings['base'], settings['quote'])
    return nil if current_price.data.nil? || bot['total_amount'].nil? || !current_price.success?

    current_value = bot['total_amount'].to_f * current_price.data.to_f
    [current_value.to_f > bot['total_cost'].to_f, bot['created_at']]
  end

  def split_bots_by_period(filtered_bots_map)
    [
      filtered_bots_map.filter { |x| x[1] <= Time.now - 12.month },
      filtered_bots_map.filter { |x| x[1] > Time.now - 12.month }
    ]
  end

  def current_price(exchange_id, base, quote)
    prices_dictionary[symbol_key(base, quote)] ||= begin
      market = markets_dictionary[exchange_id] ||= ExchangeApi::Markets::Get.new.call(exchange_id)
      symbol = market.symbol(base, quote)
      market.current_price(symbol)
    end
  end

  def symbol_key(base, quote)
    "#{base}-#{quote}"
  end
end
