class MakeTransaction < BaseService
  def initialize(
    exchange_api: ExchangeApi::Get.new,
    schedule_transaction: ScheduleTransaction.new,
    bots_repository: BotsRepository.new,
    transactions_repository: TransactionsRepository.new,
    api_keys_repository: ApiKeysRepository.new,
    notifications: Notifications::BotAlerts.new,
    validate_limit: Bots::Free::Validators::Limit.new
  )

    @get_exchange_api = exchange_api
    @schedule_transaction = schedule_transaction
    @bots_repository = bots_repository
    @transactions_repository = transactions_repository
    @api_keys_repository = api_keys_repository
    @notifications = notifications
    @validate_limit = validate_limit
  end

  def call(bot_id)
    bot = @bots_repository.find(bot_id)
    return Result::Failure.new if !make_transaction?(bot)

    api_key = @api_keys_repository.for_bot(bot.user_id, bot.exchange_id)
    api = @get_exchange_api.call(api_key)

    result = perform_action(api, bot)

    if result.failure?
      bot = @bots_repository.update(bot.id, status: 'stopped')
      @notifications.error_occured(
        bot: bot,
        errors: result.errors
      )
    end

    validate_limit_result = @validate_limit.call(bot.user)
    if validate_limit_result.failure?
      bot = @bots_repository.update(bot.id, status: 'stopped')
      @notifications.limit_reached(bot: bot)

      return Result::Failure.new(validate_limit_result.errors)
    end

    if [result, validate_limit_result].all?(&:success?)
      @schedule_transaction.call(bot)
    end
    result
  end

  private

  def perform_action(api, bot)
    result = if bot.buyer?
               api.buy(bot.settings)
             else
               api.sell(bot.settings)
             end

    @transactions_repository.create(transaction_params(result, bot))

    result
  end

  def transaction_params(result, bot)
    if result.success?
      result.data.slice(:offer_id, :rate, :amount).merge(
        bot_id: bot.id,
        status: :success
      )
    else
      {
        bot_id: bot.id,
        status: :failure,
        error_messages: result.errors
      }
    end
  end

  def make_transaction?(bot)
    bot.working?
  end
end
