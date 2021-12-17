class FetchOrderResult < BaseService
  def initialize(
    exchange_trader: ExchangeApi::Traders::Get.new,
    schedule_transaction: ScheduleTransaction.new,
    schedule_result_fetching: ScheduleResultFetching.new,
    unschedule_transactions: UnscheduleTransactions.new,
    bots_repository: BotsRepository.new,
    transactions_repository: TransactionsRepository.new,
    api_keys_repository: ApiKeysRepository.new,
    notifications: Notifications::BotAlerts.new,
    order_flow_helper: Helpers::OrderFlowHelper.new,
    subtract_credits: SubtractCredits.new
  )
    @get_exchange_trader = exchange_trader
    @schedule_transaction = schedule_transaction
    @schedule_result_fetching = schedule_result_fetching
    @unschedule_transactions = unschedule_transactions
    @bots_repository = bots_repository
    @transactions_repository = transactions_repository
    @api_keys_repository = api_keys_repository
    @notifications = notifications
    @subtract_credits = subtract_credits
    @order_flow_helper = order_flow_helper
  end

  def call(bot_id, result_params, fixing_price, notify: true, restart: true)
    bot = @bots_repository.find(bot_id)
    return Result::Failure.new unless bot.pending?

    result = perform_action(get_api(bot), result_params, bot, fixing_price)

    if result.success?
      bot = @bots_repository.update(bot.id, status: 'working', restarts: 0, fetch_restarts: 0)
      result = @order_flow_helper.validate_limit(bot, notify)
      @order_flow_helper.check_if_trial_ending_soon(bot, notify) # Send e-mail if ending soon
      @schedule_transaction.call(bot) if result.success?
    elsif !fetched?(result)
      bot = @bots_repository.update(bot.id, fetch_restarts: bot.fetch_restarts + 1)
      @schedule_result_fetching.call(bot, result_params, fixing_price)
      result = Result::Success.new
    elsif restart && recoverable?(result)
      bot = @bots_repository.update(bot.id, status: 'working', restarts: bot.restarts + 1, fetch_restarts: 0)
      @schedule_transaction.call(bot)
      @notifications.restart_occured(bot: bot, errors: result.errors) if notify
      result = Result::Success.new
    else
      @order_flow_helper.stop_bot(bot, notify, result.errors)
    end

    bot.reload
    result
  rescue StandardError
    @unschedule_transactions.call(bot)
    @order_flow_helper.stop_bot(bot, notify)
    raise
  end

  private

  def get_api(bot)
    api_key = @api_keys_repository.for_bot(bot.user_id, bot.exchange_id)
    @get_exchange_trader.call(api_key, bot.order_type)
  end

  def perform_action(api, result_params, bot, price)
    offer_id = get_offer_id(result_params)
    Rails.logger.info "Fetching order id: #{offer_id} for bot: #{bot.id}"
    result_params = result_params.merge(quote: bot.settings['quote'], base: bot.settings['base']) if probit?(bot)
    use_subaccount = bot.use_subaccount
    selected_subaccount = bot.selected_subaccount
    result = if already_fetched?(result_params)
               api.fetch_order_by_id(offer_id, result_params)
             elsif probit?(bot)
               api.fetch_order_by_id(offer_id, result_params)
             elsif use_subaccount
               api.fetch_order_by_id(offer_id, use_subaccount, selected_subaccount)
             else
               api.fetch_order_by_id(offer_id)
             end

    @transactions_repository.create(transaction_params(result, bot, price)) if result.success? || fetched?(result)

    if result.success?
      cost = result.data[:rate].to_f * result.data[:amount].to_f
      @subtract_credits.call(bot, cost)
      bot.reload
    end

    result
  end

  def probit?(bot)
    bot.exchange.name == 'Probit' || bot.exchange.name == 'Probit Global' || bot.exchange.name == 'ProBit Global'
  end

  def fetched?(result)
    result.data&.dig(:fetched).nil? || result.data&.dig(:fetched) == true
  end

  def recoverable?(result)
    result.data&.dig(:recoverable) == true
  end

  def transaction_params(result, bot, price = nil)
    if result.success?
      result.data.slice(:offer_id, :rate, :amount).merge(
        bot_id: bot.id,
        status: :success,
        bot_interval: bot.interval,
        bot_price: fixing_transaction?(price) ? price : bot.price,
        transaction_type: fixing_transaction?(price) ? 'FIXING' : 'REGULAR'
      )
    else
      {
        bot_id: bot.id,
        status: :failure,
        error_messages: result.errors,
        bot_interval: bot.interval,
        bot_price: fixing_transaction?(price) ? price : bot.price,
        transaction_type: fixing_transaction?(price) ? 'FIXING' : 'REGULAR'
      }
    end
  end

  def fixing_transaction?(price)
    !price.nil?
  end

  def already_fetched?(result_params)
    result_params.key?(:amount)
  end

  def get_offer_id(result_params)
    result_params.fetch(:offer_id)
  rescue StandardError
    result_params.fetch('offer_id')
  end
end
