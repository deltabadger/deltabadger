class StopBot < BaseService
  def initialize(
    bots_repository: BotsRepository.new,
    unschedule_transactions: UnscheduleTransactions.new
  )

    @bots_repository = bots_repository
    @unschedule_transactions = unschedule_transactions
  end

  def call(bot_id)
    bot = @bots_repository.find(bot_id)
    @unschedule_transactions.call(bot)
    @bots_repository.update(bot.id, status: 'stopped')

    Result::Success.new
  end
end
