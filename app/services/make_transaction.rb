class MakeTransaction < BaseService
  def initialize( # rubocop:disable Metrics/ParameterLists
    exchange_api: ExchangeApi::Get.new,
    schedule_transaction: ScheduleTransaction.new,
    schedule_transaction_retry: ScheduleTransactionRetry.new,
    bots_repository: BotsRepository.new,
    transactions_repository: TransactionsRepository.new,
    api_keys_repository: ApiKeysRepository.new,
    notifications: Notifications::BotAlerts.new,
    validate_limit: Bots::Free::Validators::Limit.new,
    validate_almost_limit: Bots::Free::Validators::AlmostLimit.new,
    subtract_credits: SubtractCredits.new
  )
    @get_exchange_api = exchange_api
    @schedule_transaction = schedule_transaction
    @schedule_transaction_retry = schedule_transaction_retry
    @bots_repository = bots_repository
    @transactions_repository = transactions_repository
    @api_keys_repository = api_keys_repository
    @notifications = notifications
    @validate_limit = validate_limit
    @validate_almost_limit = validate_almost_limit
    @subtract_credits = subtract_credits
  end

  # rubocop:disable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity
  def call(bot_id, notify: true, restart: true)
    bot = @bots_repository.find(bot_id)
    return Result::Failure.new if !make_transaction?(bot)

    # result = perform_action(get_api(bot), bot)
    result = Result::Failure.new('Something went wrong', { data: { recoverable: true }})

    if result.success?
      bot = @bots_repository.update(bot.id, status: 'working')
      result = validate_limit(bot, notify)
      @schedule_transaction.call(bot) if result.success?
    elsif restart && result.data&.dig(:retry)
      bot = @bots_repository.update(bot.id, status: 'working')
      @schedule_transaction_retry.call(bot)
    else
      bot = @bots_repository.update(bot.id, status: 'stopped')
      @notifications.error_occured(bot: bot, errors: result.errors) if notify
    end

    # result
    Result::Success.new
  end
  # rubocop:enable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity

  private

  def get_api(bot)
    api_key = @api_keys_repository.for_bot(bot.user_id, bot.exchange_id)
    @get_exchange_api.call(api_key)
  end

  def perform_action(api, bot)
    result = if bot.buyer?
               api.buy(bot.settings)
             else
               api.sell(bot.settings)
             end

    @transactions_repository.create(transaction_params(result, bot))

    if result.success?
      cost = result.data[:rate].to_f * result.data[:amount].to_f
      @subtract_credits.call(bot, cost)
      bot.reload
    end

    result
  end

  def validate_limit(bot, notify)
    validate_limit_result = @validate_limit.call(bot.user)
    if validate_limit_result.failure?
      bot = @bots_repository.update(bot.id, status: 'stopped')
      @notifications.limit_reached(bot: bot) if notify
    elsif @validate_almost_limit.call(bot.user).failure? && notify
      @notifications.limit_almost_reached(bot: bot)
    end

    validate_limit_result
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
