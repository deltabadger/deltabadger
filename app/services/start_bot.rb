class StartBot < BaseService
  def initialize(
    schedule_transaction: ScheduleTransaction.new,
    bots_repository: BotsRepository.new,
    validate_limit: Bots::Free::Validators::Limit.new
  )

    @schedule_transaction = schedule_transaction
    @bots_repository = bots_repository
    @validate_limit = validate_limit
  end

  def call(bot_id, continue_params = nil)
    bot = @bots_repository.find(bot_id)
    return Result::Success.new(bot) if bot.working?

    bot = set_start_bot_params(bot)

    validate_limit_result = @validate_limit.call(bot.user)
    if validate_limit_result.failure?
      bot = @bots_repository.update(bot.id, status: 'stopped')
      return validate_limit_result
    end

    @bots_repository.update(bot.id, status: 'pending')
    bot.reload

    @schedule_transaction.call(bot, first_transaction: true, continue_params: continue_params)

    Result::Success.new(bot)
  end

  private

  def set_start_bot_params(bot)
    @bots_repository.update(
      bot.id,
      status: 'working',
      restarts: 0,
      delay: 0,
      current_delay: 0
    )
  end
end
