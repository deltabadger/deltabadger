class RemoveBot < BaseService
  def initialize(bots_repository: BotsRepository.new)
    @bots_repository = bots_repository
  end

  def call(bot_id:, user:)
    bot = @bots_repository.by_id_for_user(user, bot_id)

    if bot.working?
      Result::Failure.new('Bot is currently working. Stop bot before removing')
    else
      @bots_repository.update(bot.id, status: 'deleted')

      bot.reload
      Result::Success.new(bot)
    end
  end
end
