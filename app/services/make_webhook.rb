class MakeWebhook < BaseService
  TRIGGER_TYPES = %w[main_bot additional_bot].freeze

  def initialize(
    exchange_trader: ExchangeApi::Traders::Get.new,
    schedule_webhook: ScheduleWebhook.new,
    fetch_order_result: FetchOrderResult.new,
    unschedule_webhooks: UnscheduleTransactions.new,
    bots_repository: BotsRepository.new,
    transactions_repository: TransactionsRepository.new,
    api_keys_repository: ApiKeysRepository.new,
    notifications: Notifications::BotAlerts.new,
    order_flow_helper: Helpers::OrderFlowHelper.new,
    update_formatter: Bots::Webhook::FormatParams::Update.new
  )
    @get_exchange_trader = exchange_trader
    @schedule_webhook = schedule_webhook
    @fetch_order_result = fetch_order_result
    @unschedule_webhooks = unschedule_webhooks
    @bots_repository = bots_repository
    @transactions_repository = transactions_repository
    @api_keys_repository = api_keys_repository
    @notifications = notifications
    @order_flow_helper = order_flow_helper
    @update_formatter = update_formatter
  end

  # rubocop:disable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity
  def call(bot_id, webhook, notify: true, restart: true)
    # byebug
    bot = @bots_repository.find(bot_id)
    return Result::Failure.new unless make_transaction?(bot)
    called_bot = called_bot(bot.settings, webhook)
    # byebug

    result = perform_action(get_api(bot), bot, called_bot)
    # byebug

    if !result&.success?
      # byebug
      triggered_types(bot, called_bot) if bot.first_time?
      settings_params = @update_formatter.call(bot, bot.settings.merge(user: bot.user))
      @bots_repository.update(bot.id, **settings_params.merge(success_status(bot)))
      result = @fetch_order_result.call(bot.id, result.data, bot.price.to_f)
      send_user_to_sendgrid(bot)
    elsif restart && recoverable?(result)
      # byebug
      result = range_check_result if result.nil?
      @transactions_repository.create(failed_transaction_params(result, bot))
      bot = @bots_repository.update(bot.id, status: 'working', restarts: bot.restarts + 1, fetch_restarts: 0)
      @schedule_webhook.call(bot, webhook)
      @notifications.restart_occured(bot: bot, errors: result.errors) if notify
      result = Result::Success.new
    else
      # byebug
      @transactions_repository.create(failed_transaction_params(result, bot))
      @notifications.error_occured(bot: bot, errors: result.errors) if notify
    end

    result
  rescue => e
    @unschedule_webhooks.call(bot)
    @order_flow_helper.stop_bot(bot, notify)
    # byebug
    Rails.logger.info "======================= RESCUE 1=============================="
    Rails.logger.info "================= #{e.inspect} ======================="
    Rails.logger.info "====================================================="

    raise
  end
  # rubocop:enable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity

  def success_status(bot)
    return {status: 'working'} if bot.every_time? || !bot.already_triggered_types.blank?
    {status: 'stopped'}
  end

  def called_bot(settings, webhook)
    return "additional_bot" if settings["additional_type_enabled"] && settings["additional_trigger_url"] == webhook
    "main_bot" if settings["trigger_url"] == webhook
  end

  def triggered_types(bot, called_bot)
    bot.already_triggered_types |= [called_bot]
    bot.already_triggered_types = [] if (TRIGGER_TYPES - bot.already_triggered_types).blank?
  end

  private

  def send_user_to_sendgrid(bot)
    return unless bot.successful_transaction_count == 1
    api = get_api(bot)
    api.send_user_to_sendgrid(bot.exchange.name, bot.user)
  end

  def check_allowable_balance(api, bot)
    balance = api.currency_balance(bot.quote)
    return unless balance.success?

    balance.data
  end

  def get_api(bot)
    api_key = @api_keys_repository.for_bot(bot.user_id, bot.exchange_id)
    @get_exchange_trader.call(api_key, bot.order_type)
  end

  def transaction_settings(bot, type, called_bot)
    # byebug
    {
      base: bot.base,
      quote: bot.quote,
      price: calculate_price(bot, type, called_bot)
    }.compact
  end

  def perform_action(api, bot, called_bot)
    # byebug
    type = bot.settings[called_bot == 'additional_bot'? 'additional_type' : 'type']
    settings = transaction_settings(bot, type, called_bot)
    result = if buyer?(type)
               # byebug
               api.buy(**settings)
             else
               # byebug
               api.sell(**settings)
             end

    result
  end

  def calculate_price(bot, type, called_bot)
    # byebug
    case type
    when 'buy', 'sell'
      # byebug
      bot.send(called_bot == 'additional_bot'? 'additional_price' : 'price').to_f
    when 'buy_all', 'sell_all'
      # byebug
      allowable_balance(get_api(bot), bot).to_f
    else
      # byebug
      0.0
    end
  end

  def allowable_balance(api, bot)
    @allowable_balance ||= check_allowable_balance(api, bot)
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

  def failed_transaction_params(result, bot)
    {
      bot_id: bot.id,
      status: :failure,
      error_messages: result.errors,
      bot_price: bot.price,
      transaction_type: 'REGULAR'
    }
  end

  def buyer?(type)
    type.in?(%w(buy buy_all))
  end
end
