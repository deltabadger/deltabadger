class StartBot < BaseService
  def initialize(
    make_transaction: MakeTransaction.new,
    bots_repository: BotsRepository.new,
    validate_limit: Bots::Free::Validators::Limit.new
  )

    @make_transaction = make_transaction
    @bots_repository = bots_repository
    @validate_limit = validate_limit
  end

  def call(bot_id)
    bot = @bots_repository.find(bot_id)
    return Result::Success.new(bot) if bot.working?

    bot = set_start_bot_params(bot)

    validate_limit_result = @validate_limit.call(bot.user)
    if validate_limit_result.failure?
      bot = @bots_repository.update(bot.id, status: 'stopped')
      return validate_limit_result
    end

    result = @make_transaction.call(bot.id, notify: false, restart: false)

    @bots_repository.update(bot.id, status: 'stopped') if result.failure?

    bot.reload

    if result.success?
      Result::Success.new(bot)
    else
      Result::Failure.new(*result.errors)
    end
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
