class SetUpSidekiq
  def initialize(
    schedule_transaction: ScheduleTransaction.new,
    schedule_withdrawal: ScheduleWithdrawal.new
  )
    @schedule_transaction = schedule_transaction
    @schedule_withdrawal = schedule_withdrawal
  end

  def fill_sidekiq_queue(schedule: true)
    Bot.scheduled.each do |bot|
      if bot.basic?
        params = continue_params(bot)
        if params.present? && (params[:price] - bot.settings['price'].to_f).positive?
          puts "User: #{bot.user.id}, bot: #{bot.id}, email: #{bot.user.email}, missed amount: #{params[:price].to_f}, bot settings: #{bot.settings}"
        end
        @schedule_transaction.call(bot) if schedule

        # disabled for now, must verify the continue_params[:price] is properly used (eg: buy 0.5 € not 0.5 BTC)
        # @schedule_transaction.call(bot, continue_params: params) if schedule

      elsif bot.withdrawal?
        puts "User: #{bot.user.id}, bot: #{bot.id},  email: #{bot.user.email}, missed withdrawals: #{missed_withdrawals(bot)}, bot settings: #{bot.settings}"
        @schedule_withdrawal.call(bot) if schedule
      end
    end

    true
  end

  private

  def missed_withdrawals(bot)
    next_withdrawal_at = NextWithdrawalBotTransactionAt.new.call(bot)
    interval = ParseInterval.new.call(bot).to_i
    time_missing_transactions = Time.current - next_withdrawal_at
    (time_missing_transactions.to_f / interval).floor
  rescue StandardError
    'unknown'
  end

  def continue_params(bot)
    restart_params = GetRestartParams.new.call(bot_id: bot.id)
    return unless restart_params[:restartType] == 'missed'

    { price: restart_params[:missedAmount], continue_schedule: false }
  end
end
