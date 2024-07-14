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
        restart_params = GetRestartParams.new.call(bot.id)
        if restart_params[:restartType] == 'missed'
          puts "User: #{bot.user.id} bot: #{bot.id} missed transactions amount #{restart_params[:missedAmount]}"
          continue_params = {price: restart_params[:missedAmount], continue_schedule: false}
        else
          puts "User: #{bot.user.id} bot: #{bot.id} not missed transactions"
          continue_params = nil
        end
        # @schedule_transaction.call(bot, continue_params: continue_params)
      elsif bot.withdrawal?
        puts "User: #{bot.user.id} bot: #{bot.id} missed withdrawals: #{missed_withdrawals(bot)}"
        # @schedule_withdrawal.call(bot)
      end
    end

    true
  end

  private

  def working?(bot)
    bot.status == 'working'
  end

  def missed_withdrawals(bot)
    next_withdrawal_at = NextWithdrawalBotTransactionAt.new.call(bot)
    interval = ParseInterval.new.call(bot).to_i
    time_missing_transactions = Time.current - next_withdrawal_at
    (time_missing_transactions.to_f / interval).floor
  end
end
