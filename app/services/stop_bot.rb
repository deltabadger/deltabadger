class StopBot < BaseService
  def initialize(
    bots_repository: BotsRepository.new
  )

    @bots_repository = bots_repository
  end

  def call(bot_id)
    bot = @bots_repository.find(bot_id)
    @bots_repository.update(bot.id, status: 'stopped')

    Result::Success.new
  end
end
