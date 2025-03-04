class StartBot < BaseService
  def initialize(
    schedule_transaction: ScheduleTransaction.new,
    schedule_withdrawal: ScheduleWithdrawal.new
  )
    @schedule_transaction = schedule_transaction
    @schedule_withdrawal = schedule_withdrawal
  end

  def call(bot_id, continue_params = nil)
    bot = Bot.find(bot_id)
    return Result::Success.new(bot) if bot.working?

    start_params = {
      status: bot.basic? ? 'pending' : 'working',
      restarts: 0,
      delay: 0,
      current_delay: 0
    }
    bot.update(start_params)

    if bot.basic?
      @schedule_transaction.call(bot, first_transaction: true, continue_params: continue_params)
    elsif bot.withdrawal?
      @schedule_withdrawal.call(bot, first_transaction: true)
    end

    Result::Success.new(bot)
  end
end
