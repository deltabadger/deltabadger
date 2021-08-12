require 'json'

class MetricsController < ApplicationController
  TOP_BOTS_KEY = 'TOP_BOTS_CACHE_KEY'.freeze
  def index
    telegram_metrics = FetchTelegramMetrics.new.call

    output_params = {
      liveBots: BotsRepository.new.count_with_status('working'),
      btcBought: convert_to_satoshis(TransactionsRepository.new.total_btc_bought),
      btcBoughtDayAgo: convert_to_satoshis(TransactionsRepository.new.total_btc_bought_day_ago)
    }.merge(telegram_metrics)

    render json: { data: output_params }.to_json
  end

  def top_ten_bots
    Rails.cache.exist?(TOP_BOTS_KEY) ? (render json: Rails.cache.read(TOP_BOTS_KEY).to_json) : top_bots_update
  end

  def top_bots_update
    top_ten_bots = BotsRepository.new.list_top_ten
    new_top_bots = top_ten_bots.map do |key, value|
      BotCount.new(key, value)
    end
    old_top_bots = Rails.cache.exist?(TOP_BOTS_KEY) ? Rails.cache.read(TOP_BOTS_KEY) : new_top_bots
    new_top_bots.each_with_index do |new_bot, new_index|
      found = false
      old_top_bots.each_with_index do |old_bot, old_index|
        next unless new_bot.name == old_bot.name

        found = true
        new_bot.is_up = true if new_index < old_index
      end
      puts found
      new_bot.is_up = true unless found
    end
    Rails.cache.write(TOP_BOTS_KEY, new_top_bots, expires_in: 25.hour)
    render json: new_top_bots.to_json
  end

  private

  class BotCount
    attr_reader :name, :count, :is_up
    attr_writer :is_up
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

