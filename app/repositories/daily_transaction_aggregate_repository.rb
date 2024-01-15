class DailyTransactionAggregateRepository < BaseRepository
  BTC = %w[XXBT XBT BTC].freeze

  def for_bot(bot, limit: nil)
    bot.daily_transaction_aggregates.limit(limit).order(id: :desc)
  end

  def today_for_bot(bot)
    for_bot(bot).where('created_at >= ?', Date.today.beginning_of_day)
  end

  def for_bot_by_status(bot, limit: nil, status: :success)
    bot.daily_transaction_aggregates.where(status: status).limit(limit).order(created_at: :desc)
  end

  def count_by_status_and_exchange(status, exchange)
    model.joins(:bot).where(bots: { exchange_id: exchange.id }).where(status: status).count
  end

  def total_btc_bought
    ActiveRecord::Base.connection.execute("select sum(total_amount) from bots_total_amounts where settings->>'base' in ('XXBT','XBT','BTC')")[0]['sum'].to_f
  end

  def total_btc_sold
    total_btc_by_type('sell')
  end

  def total_btc_bought_day_ago
    model.joins(:bot)
         .where("bots.settings->>'type' = 'buy' AND bots.settings->>'base' IN (?) AND daily_transaction_aggregates.created_at < ? ",
                BTC, 1.days.ago)
         .sum(:amount).ceil(8)
  end

  def model
    DailyTransactionAggregate
  end

  private

  def total_btc_by_type(type)
    model.joins(:bot)
         .where("bots.settings->>'type' = ? AND bots.settings->>'base' IN (?)", type, BTC)
         .sum(:amount).ceil(8)
  end
end
