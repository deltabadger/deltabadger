require 'json'
class MetricsController < ApplicationController
  def index
    return render json: { data: Rails.cache.read(ENV.fetch('METRICS_CACHE_KEY')) }.to_json if Rails.cache.exist?(ENV.fetch('METRICS_CACHE_KEY'))

    telegram_metrics = FetchTelegramMetrics.new.call

    output_params = {
      liveBots: BotsRepository.new.count_with_status('working'),
      btcBought: MetricsRepository.new.convert_to_satoshis(TransactionsRepository.new.total_btc_bought),
      btcBoughtDayAgo: MetricsRepository.new.convert_to_satoshis(TransactionsRepository.new.total_btc_bought_day_ago)
    }.merge(telegram_metrics)
    Rails.cache.write(ENV.fetch('METRICS_CACHE_KEY'), output_params, expires_in: 1.5.minute)
    render json: { data: output_params }.to_json
  end
end
