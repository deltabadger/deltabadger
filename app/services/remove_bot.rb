class RemoveBot < BaseService
  def initialize(bots_repository: BotsRepository.new, unschedule_transactions: UnscheduleTransactions.new)
    @bots_repository = bots_repository
    @unschedule_transactions = unschedule_transactions
  end

  def call(bot_id:, user:)
    bot = @bots_repository.by_id_for_user(user, bot_id)

    if bot.working?
      Result::Failure.new('Bot is currently working. Stop bot before removing')
    else
      @unschedule_transactions.call(bot)
      @bots_repository.destroy(bot.id)

      bot.reload
      Result::Success.new(bot)
    end
  end
end
