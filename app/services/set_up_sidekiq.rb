class SetUpSidekiq
  def initialize(
    schedule_transaction: ScheduleTransaction.new,
    schedule_withdrawal: ScheduleWithdrawal.new
  )
    @schedule_transaction = schedule_transaction
    @schedule_withdrawal = schedule_withdrawal
  end

  def fill_sidekiq_queue
    Bot.working.each do |bot|
      info = missing_transactions_info(bot)
      puts info if info.present?
      @schedule_transaction.call(bot) if bot.trading?
      @schedule_withdrawal.call(bot) if bot.withdrawal?
    end

    true
  end

  private

  def working?(bot)
    bot.status == 'working'
  end

  def missing_transactions_info(bot)
    return unless bot.trading? || bot.withdrawal?

    interval = ParseInterval.new.call(bot).to_i
    next_transaction_at = bot.trading? ? NextTradingBotTransactionAt.new.call(bot) : NextWithdrawalBotTransactionAt.new.call(bot)
    time_missing_transactions = Time.current - next_transaction_at
    missed_transactions = (time_missing_transactions.to_f / interval).floor
    return if missed_transactions.zero?

    "User: #{bot.user.id} bot: #{bot.id} missed transactions: #{missed_transactions}"
  end
end
