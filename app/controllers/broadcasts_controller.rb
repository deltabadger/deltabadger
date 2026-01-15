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

  # Called when app becomes visible after being in background/sleep
  # Releases any overdue scheduled jobs that were missed while inactive
  # and broadcasts status bar updates for all working bots
  def wake_dispatcher
    released_count = 0

    SolidQueue::ScheduledExecution
      .where("scheduled_at <= ?", Time.current)
      .limit(100)
      .each do |execution|
        execution.promote
        released_count += 1
      end

    Rails.logger.info "[WakeDispatcher] Released #{released_count} overdue job(s)" if released_count > 0

    # Broadcast status bar updates for all working bots after a delay
    # This ensures the UI refreshes even if the WebSocket wasn't ready during job execution
    current_user.bots.working.find_each do |bot|
      Bot::BroadcastAfterScheduledActionJob.set(wait: 1.second).perform_later(bot)
    end

    head :ok
  end
end
