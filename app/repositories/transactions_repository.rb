class TransactionsRepository < BaseRepository
  def for_bot(bot, limit: nil)
    bot.transactions.limit(limit)
  end

  def successful_for_bot(bot, limit: nil)
    bot.transactions.where(status: :success).limit(limit)
  end

  def model
    Transaction
  end
end
