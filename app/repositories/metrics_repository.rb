class MetricsRepository < BaseRepository
  def update_metrics
    telegram_metrics = FetchTelegramMetrics.new.call

    output_params = {
      liveBots: BotsRepository.new.count_with_status('working'),
      btcBought: convert_to_satoshis(TransactionsRepository.new.total_btc_bought),
      btcBoughtDayAgo: convert_to_satoshis(TransactionsRepository.new.total_btc_bought_day_ago)
    }.merge(telegram_metrics)
    Rails.cache.write(ENV.fetch('METRICS_CACHE_KEY'), output_params, expires_in: 2.minute)
  end

  def convert_to_satoshis(amount)
    (amount * 10**8).ceil
  end
end
