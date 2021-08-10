require 'json'

class MetricsController < ApplicationController
  def index
    telegram_metrics = FetchTelegramMetrics.new.call

    output_params = {
      liveBots: BotsRepository.new.count_with_status('working'),
      btcBought: convert_to_satoshis(TransactionsRepository.new.total_btc_bought),
      btcBoughtDayAgo: convert_to_satoshis(TransactionsRepository.new.total_btc_bought_day_ago)
    }.merge(telegram_metrics)

    render json: { data: output_params }.to_json
  end

  def top_five_bots
    top_five_bots = BotsRepository.new.list_top_five
    bots_array = top_five_bots.map do |key, value|
      BotCount.new(key, value)
    end
    render json: bots_array.to_json
  end

  private

  class BotCount
    def initialize(name, count, is_up = false)
      @name = name
      @count = count
      @is_up = is_up
    end
  end

  def convert_to_satoshis(amount)
    (amount * 10**8).ceil
  end
end

