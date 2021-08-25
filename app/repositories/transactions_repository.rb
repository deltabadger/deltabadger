class TransactionsRepository < BaseRepository
  BTC = %w[XXBT XBT BTC].freeze

  def for_bot(bot, limit: nil)
    bot.transactions.limit(limit).order(id: :desc)
  end

  def for_bot_by_status(bot, limit: nil, status: :success)
    bot.transactions.where(status: status).limit(limit).order(created_at: :desc)
  end

  def count_by_status_and_exchange(status, exchange)
    model.joins(:bot).where(bots: { exchange_id: exchange.id }).where(status: status).count
  end

  def total_btc_bought
    total_btc_by_type('buy')
  end

  def total_btc_sold
    total_btc_by_type('sell')
  end

  def total_btc_bought_day_ago
    model.joins(:bot)
      .where("bots.settings->>'type' = 'buy' AND bots.settings->>'base' IN (?) AND transactions.created_at < ? ",
             BTC, 1.days.ago)
      .sum(:amount).ceil(8)
  end

  def model
    Transaction
  end

  private

  def total_btc_by_type(type)
    model.joins(:bot)
         .where("bots.settings->>'type' = ? AND bots.settings->>'base' IN (?)", type, BTC)
         .sum(:amount).ceil(8)
  end
end
