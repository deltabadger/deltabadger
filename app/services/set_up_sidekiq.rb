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
      if bot.trading?
        next_transaction_at = NextTradingBotTransactionAt.new.call(bot)
        missed_transactions(bot, next_transaction_at).times do |i|
          puts "User: #{bot.user.id} bot: #{bot.id} manually setting missed transaction #{i}"
          @schedule_transaction.call(bot)
        end
        @schedule_transaction.call(bot)
      elsif bot.withdrawal?
        next_transaction_at = NextWithdrawalBotTransactionAt.new.call(bot)
        missed_transactions(bot, next_transaction_at).times do |i|
          puts "User: #{bot.user.id} bot: #{bot.id} manually setting missed withdrawal #{i}"
          @schedule_withdrawal.call(bot)
        end
        @schedule_withdrawal.call(bot)
      end
    end

    true
  end

  private

  def working?(bot)
    bot.status == 'working'
  end

  def missed_transactions(bot, next_transaction_at)
    interval = ParseInterval.new.call(bot).to_i
    time_missing_transactions = Time.current - next_transaction_at
    (time_missing_transactions.to_f / interval).floor
  end
end
