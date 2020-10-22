class TransactionsRepository < BaseRepository
  BTC = %w[XXBT XBT BTC].freeze

  def for_bot(bot, limit: nil)
    bot.transactions.limit(limit).order(id: :desc)
  end

  def successful_for_bot(bot, limit: nil)
    bot.transactions.where(status: :success).limit(limit).order(id: :desc)
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

  def model
    Transaction
  end

  private

  def total_btc_by_type(type)
    model.joins(:bot)
         .where("bots.settings->>'type' = ? AND bots.settings->>'base' IN (?)", type, BTC)
         .sum(:amount)
  end
end
