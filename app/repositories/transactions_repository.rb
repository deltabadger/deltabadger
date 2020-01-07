class TransactionsRepository < BaseRepository
  def for_bot(bot, limit: nil)
    bot.transactions.limit(limit).order(id: :desc)
  end

  def successful_for_bot(bot, limit: nil)
    bot.transactions.where(status: :success).limit(limit).order(id: :desc)
  end

  def count_by_status_and_exchange(status, exchange)
    Transaction.joins(:bot).where('bots.exchange_id = ?', exchange.id).where(status: status).count
  end

  def model
    Transaction
  end
end
