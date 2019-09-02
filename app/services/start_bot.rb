class StartBot < BaseService
  def initialize(
    schedule_transaction: ScheduleTransaction.new,
    bots_repository: BotsRepository.new
  )

    @schedule_transaction = schedule_transaction
    @bots_repository = bots_repository
  end

  def call(bot_id)
    bot = @bots_repository.find(bot_id)
    @schedule_transaction.call(bot)
    @bots_repository.update(bot.id, status: 'working')

    Result::Success.new
  end
end
