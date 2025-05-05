class RemoveBot < BaseService
  def initialize(unschedule_transactions: UnscheduleTransactions.new)
    @unschedule_transactions = unschedule_transactions
  end

  def call(bot_id:, user:)
    bot = user.bots.find(bot_id)
    return Result::Failure.new('Bot not found') unless bot

    if bot.working?
      Result::Failure.new('Bot is currently working. Stop bot before removing')
    else
      @unschedule_transactions.call(bot)
      bot.destroy

      Result::Success.new(bot)
    end
  end
end
