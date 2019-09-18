class StartBot < BaseService
  def initialize(
    make_transaction: MakeTransaction.new,
    bots_repository: BotsRepository.new
  )

    @make_transaction = make_transaction
    @bots_repository = bots_repository
  end

  def call(bot_id)
    bot = @bots_repository.find(bot_id)
    @bots_repository.update(bot.id, status: 'working')
    result = @make_transaction.call(bot.id)

    @bots_repository.update(bot.id, status: 'stopped') if result.failure?

    result
  end
end
