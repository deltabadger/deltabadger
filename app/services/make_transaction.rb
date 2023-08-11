class MakeTransaction < BaseService
  def initialize(
    exchange_trader: ExchangeApi::Traders::Get.new,
    schedule_transaction: ScheduleTransaction.new,
    fetch_order_result: FetchOrderResult.new,
    unschedule_transactions: UnscheduleTransactions.new,
    bots_repository: BotsRepository.new,
    transactions_repository: TransactionsRepository.new,
    api_keys_repository: ApiKeysRepository.new,
    notifications: Notifications::BotAlerts.new,
    order_flow_helper: Helpers::OrderFlowHelper.new,
    check_price_range: CheckPriceRange.new
  )
    @get_exchange_trader = exchange_trader
    @schedule_transaction = schedule_transaction
    @fetch_order_result = fetch_order_result
    @unschedule_transactions = unschedule_transactions
    @bots_repository = bots_repository
    @transactions_repository = transactions_repository
    @api_keys_repository = api_keys_repository
    @notifications = notifications
    @order_flow_helper = order_flow_helper
    @check_price_range = check_price_range
  end

  # rubocop:disable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity
  def call(bot_id, notify: true, restart: true, continue_params: nil)
    bot = @bots_repository.find(bot_id)
    return Result::Failure.new unless make_transaction?(bot)

    continue_params = extract_continue_params(continue_params)

    continue_schedule = continue_params['continue_schedule']
    fixing_price = continue_params['price']
    range_check_result = @check_price_range.call(bot)

    if range_check_result.success? && !range_check_result.data[:valid]
      continue_schedule = true
      skip_if_not_in_range(bot, range_check_result.data)
    end

    result = perform_action(get_api(bot), bot, fixing_price) unless continue_schedule || !range_check_result.success?

    if continue_schedule
      bot = @bots_repository.update(bot.id, status: 'working', restarts: 0)
      result = @order_flow_helper.validate_limit(bot, notify)
      @order_flow_helper.check_if_trial_ending_soon(bot, notify) # Send e-mail if ending soon
      @schedule_transaction.call(bot) if result.success?
    elsif result&.success?
      @bots_repository.update(bot.id, status: 'pending')
      result = @fetch_order_result.call(bot.id, result.data, fixing_price)
      check_allowable_balance(get_api(bot), bot, fixing_price, notify)
      send_user_to_sendgrid(bot)
    elsif restart && (!range_check_result.success? || recoverable?(result))
      result = range_check_result if result.nil?
      @transactions_repository.create(failed_transaction_params(result, bot, fixing_price))
      bot = @bots_repository.update(bot.id, status: 'working', restarts: bot.restarts + 1, fetch_restarts: 0)
      @schedule_transaction.call(bot)
      @notifications.restart_occured(bot: bot, errors: result.errors) if notify
      result = Result::Success.new
    else
      @transactions_repository.create(failed_transaction_params(result, bot, fixing_price))
      @order_flow_helper.stop_bot(bot, notify, result.errors)
    end

    result
  rescue => e
    @unschedule_transactions.call(bot)
    @order_flow_helper.stop_bot(bot, notify)
    Rails.logger.info "======================= RESCUE 1 MakeTransaction =============================="
    Rails.logger.info "================= #{e.inspect} ======================="
    Rails.logger.info "====================================================="

    raise
  end
  # rubocop:enable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity

  private

  def send_user_to_sendgrid(bot)
    return unless bot.successful_transaction_count == 1
    api = get_api(bot)
    api.send_user_to_sendgrid(bot.exchange.name, bot.user)
  end

  def check_allowable_balance(api, bot, price = nil, notify = true)
    price = fixing_transaction?(price) ? price.to_f : bot.price.to_f
    balance = api.currency_balance(bot.quote)
    return unless balance.success?

    amount_needed = calculate_amount_needed(bot.interval, price, bot.force_smart_intervals)
    @notifications.end_of_funds(bot: bot) if balance.data.to_f < amount_needed && notify
  end

  def get_api(bot)
    api_key = @api_keys_repository.for_bot(bot.user_id, bot.exchange_id)
    @get_exchange_trader.call(api_key, bot.order_type)
  end

  def perform_action(api, bot, price = nil)
    settings = {
      base: bot.base,
      quote: bot.quote,
      price: fixing_transaction?(price) ? price.to_f : bot.price.to_f,
      percentage: (bot.percentage.to_f if bot.limit?),
      force_smart_intervals: fixing_transaction?(price) ? false : bot.force_smart_intervals,
      smart_intervals_value: bot.smart_intervals_value.to_f
    }.compact
    subaccount_settings = {
      use_subaccount: bot.use_subaccount,
      selected_subaccount: bot.selected_subaccount
    }.compact
    settings = settings.merge(subaccount_settings) if bot.use_subaccount
    result = if bot.buyer?
               api.buy(**settings)
             else
               is_legacy = bot.type == 'sell_old'
               api.sell(**settings.merge(is_legacy: is_legacy))
             end

    result
  end

  def extract_continue_params(continue_params)
    return { continue_schedule: false, price: nil } if continue_params.nil?

    return eval(continue_params) if continue_params.instance_of?(String)

    continue_params
  end

  def skip_if_not_in_range(bot, result)
    transaction_params = {
      bot_id: bot.id,
      status: :skipped,
      rate: result[:rate],
      amount: result[:amount],
      bot_interval: bot.interval,
      bot_price: bot.price,
      transaction_type: 'REGULAR'
    }

    @transactions_repository.create(transaction_params)
  end

  def fixing_transaction?(price)
    !price.nil?
  end

  def make_transaction?(bot)
    bot.working? || bot.pending?
  end

  def recoverable?(result)
    result.data&.dig(:recoverable) == true
  end

  def failed_transaction_params(result, bot, price = nil)
    {
      bot_id: bot.id,
      status: :failure,
      error_messages: result.errors,
      bot_interval: bot.interval,
      bot_price: fixing_transaction?(price) ? price : bot.price,
      transaction_type: fixing_transaction?(price) ? 'FIXING' : 'REGULAR'
    }
  end

  def calculate_amount_needed(interval, price, smart_intervals)
    case interval
    when 'hour'
      price * 24 * 3
    when 'day'
      price * 3
    when 'week'
      smart_intervals ? price / 7 * 3 : price # price / 7 * 3 = aprox. 3 days
    else
      smart_intervals ? price / 30 * 3 : price # price / 30 * 3 = aprox. 3 days
    end
  end
end
