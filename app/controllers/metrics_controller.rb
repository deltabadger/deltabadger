require 'json'

class MetricsController < ApplicationController
  METRICS_CACHE_KEY = 'METRICS_CACHE_KEY'.freeze
  def index
    return render json: { data: Rails.cache.read(METRICS_CACHE_KEY) }.to_json if Rails.cache.exist?(METRICS_CACHE_KEY)

    telegram_metrics = FetchTelegramMetrics.new.call

    output_params = {
      liveBots: BotsRepository.new.count_with_status('working'),
      btcBought: convert_to_satoshis(TransactionsRepository.new.total_btc_bought),
      btcBoughtDayAgo: convert_to_satoshis(TransactionsRepository.new.total_btc_bought_day_ago)
    }.merge(telegram_metrics)
    Rails.cache.write(METRICS_CACHE_KEY,output_params,expires_in: 1.minute)
    render json: { data: output_params }.to_json
  end

  private

  def convert_to_satoshis(amount)
    (amount * 10**8).ceil
  end
end
