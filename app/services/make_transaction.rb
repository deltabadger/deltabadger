class MakeTransaction < BaseService
  def initialize(
    exchange_api: ExchangeApi::Get.new,
    schedule_transaction: ScheduleTransaction.new,
    bots_repository: BotsRepository.new,
    transactions_repository: TransactionsRepository.new,
    api_keys_repository: ApiKeysRepository.new
  )

    @get_exchange_api = exchange_api
    @schedule_transaction = schedule_transaction
    @bots_repository = bots_repository
    @transactions_repository = transactions_repository
    @api_keys_repository = api_keys_repository
  end

  def call(bot_id)
    bot = @bots_repository.find(bot_id)
    api_key = @api_keys_repository.for_bot(bot.user_id, bot.exchange_id)
    api = @get_exchange_api.call(api_key)

    return false if !make_transaction?(bot)

    perform_action(api, bot)
    @schedule_transaction.call(bot)
  end

  private

  def perform_action(api, bot)
    result = if bot.buyer?
               api.buy(bot.settings)
             else
               api.sell(bot.settings)
             end

    @transactions_repository.create(
      result.data.merge(
        bot_id: bot.id,
        currency: bot.currency,
        status: result.success? ? :success : :failure
      )
    )
  end

  def make_transaction?(bot)
    bot.working?
  end
end
