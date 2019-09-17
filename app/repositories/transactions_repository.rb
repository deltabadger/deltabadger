class TransactionsRepository < BaseRepository
  def for_bot(bot)
    bot.transactions
  end

  def model
    Transaction
  end
end
