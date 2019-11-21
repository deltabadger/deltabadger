class TransactionsRepository < BaseRepository
  def for_bot(bot, limit: nil)
    bot.transactions.limit(limit).order(id: :desc)
  end

  def successful_for_bot(bot, limit: nil)
    bot.transactions.where(status: :success).limit(limit).order(id: :desc)
  end

  def model
    Transaction
  end
end
