class SetUpSidekiq
  def initialize(
    schedule_transaction: ScheduleTransaction.new,
    schedule_withdrawal: ScheduleWithdrawal.new
  )
    @schedule_transaction = schedule_transaction
    @schedule_withdrawal = schedule_withdrawal
  end

  def fill_sidekiq_queue(dry_run: false)
    Bot.working.each do |bot|
      if bot.trading?
        params = continue_params(bot)
        puts "User: #{bot.user.id} bot: #{bot.id} missed transactions amount: #{params}"
        @schedule_transaction.call(bot, continue_params: params) unless dry_run
      elsif bot.withdrawal?
        puts "User: #{bot.user.id} bot: #{bot.id} missed withdrawals: #{missed_withdrawals(bot)}"
        @schedule_withdrawal.call(bot) unless dry_run
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

  def continue_params(bot)
    restart_params = GetRestartParams.new.call(bot.id)
    if restart_params[:restartType] == 'missed'
      puts "User: #{bot.user.id} bot: #{bot.id} missed transactions amount #{restart_params[:missedAmount]}"
      { price: restart_params[:missedAmount], continue_schedule: false }
    else
      puts "User: #{bot.user.id} bot: #{bot.id} not missed transactions"
      nil
    end
  end
end
