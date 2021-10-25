class NextWithdrawalBotTransactionAt < BaseService
  def call(bot, first_transaction: false)
    return DateTime.now if first_transaction

    interval = bot.interval_enabled ? bot.interval.to_f * 1.day : 1.day
    interval.since(bot.last_withdrawal.created_at)
  end
end
