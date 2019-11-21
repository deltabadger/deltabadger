class MakeTransaction < BaseService
  def initialize(
    exchange_api: ExchangeApi::Get.new,
    schedule_transaction: ScheduleTransaction.new,
    bots_repository: BotsRepository.new,
    transactions_repository: TransactionsRepository.new,
    api_keys_repository: ApiKeysRepository.new,
    notifications: Notifications::BotAlerts.new
  )

    @get_exchange_api = exchange_api
    @schedule_transaction = schedule_transaction
    @bots_repository = bots_repository
    @transactions_repository = transactions_repository
    @api_keys_repository = api_keys_repository
    @notifications = notifications
  end

  def call(bot_id)
    bot = @bots_repository.find(bot_id)
    api_key = @api_keys_repository.for_bot(bot.user_id, bot.exchange_id)
    api = @get_exchange_api.call(api_key)

    return false if !make_transaction?(bot)

    result = perform_action(api, bot)

    @schedule_transaction.call(bot) if result.success?
    if result.failure?
      bot = @bots_repository.update(bot.id, status: 'stopped')
      @notifications.error_occured(
        bot: bot,
        user: bot.user,
        errors: result.errors
      )
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
