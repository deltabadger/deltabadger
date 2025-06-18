class BroadcastsController < ApplicationController
  def metrics_update
    bot = Bot.find(params['bot_id'])
    return if bot.nil?

    Bot::BroadcastMetricsUpdateJob.perform_later(bot)
    head :ok
  end

  def pnl_update
    bots = Bot.where(id: params['bot_ids'])
    bots.each do |bot|
      Bot::BroadcastPnlUpdateJob.perform_later(bot)
    end
    head :ok
  end

  def price_limit_info_update
    bot = Bot.find_by(id: params['bot_id'])
    return if bot.nil?

    Bot::BroadcastPriceLimitInfoUpdateJob.perform_later(bot)
    head :ok
  end

  def price_drop_limit_info_update
    bot = Bot.find_by(id: params['bot_id'])
    return if bot.nil?

    Bot::BroadcastPriceDropLimitInfoUpdateJob.perform_later(bot)
    head :ok
  end

  def indicator_limit_info_update
    bot = Bot.find_by(id: params['bot_id'])
    return if bot.nil?

    Bot::BroadcastIndicatorLimitInfoUpdateJob.perform_later(bot)
    head :ok
  end

  def moving_average_limit_info_update
    bot = Bot.find_by(id: params['bot_id'])
    return if bot.nil?

    Bot::BroadcastMovingAverageLimitInfoUpdateJob.perform_later(bot)
    head :ok
  end

  def fetch_open_orders
    bot = Bot.find_by(id: params['bot_id'])
    return if bot.nil?

    Bot::FetchAndUpdateOpenOrdersJob.perform_later(bot, update_missed_quote_amount: true, success_or_kill: true)
    head :ok
  end
end
