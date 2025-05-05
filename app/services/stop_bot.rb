class StopBot < BaseService
  def initialize(
    unschedule_transactions: UnscheduleTransactions.new
  )
    @unschedule_transactions = unschedule_transactions
  end

  def call(bot_id)
    bot = Bot.find(bot_id)
    return Result::Failure.new('Bot not found') unless bot.present?

    @unschedule_transactions.call(bot)
    bot.update(status: 'stopped')

    Result::Success.new(bot)
  end
end
