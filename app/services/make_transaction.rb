# rubocop:disable Metrics/ClassLength
class MakeTransaction < BaseService
  def initialize( # rubocop:disable Metrics/ParameterLists
    exchange_trader: ExchangeApi::Traders::Get.new,
    schedule_transaction: ScheduleTransaction.new,
    unschedule_transactions: UnscheduleTransactions.new,
    bots_repository: BotsRepository.new,
    transactions_repository: TransactionsRepository.new,
    api_keys_repository: ApiKeysRepository.new,
    notifications: Notifications::BotAlerts.new,
    validate_limit: Bots::Free::Validators::Limit.new,
    validate_almost_limit: Bots::Free::Validators::AlmostLimit.new,
    validate_trial_ending_soon: Bots::Free::Validators::TrialEndingSoon.new,
    subtract_credits: SubtractCredits.new
  )
    @get_exchange_trader = exchange_trader
    @schedule_transaction = schedule_transaction
    @unschedule_transactions = unschedule_transactions
    @bots_repository = bots_repository
    @transactions_repository = transactions_repository
    @api_keys_repository = api_keys_repository
    @notifications = notifications
    @validate_limit = validate_limit
    @validate_almost_limit = validate_almost_limit
    @validate_trial_ending_soon = validate_trial_ending_soon
    @subtract_credits = subtract_credits
  end

  # rubocop:disable Metrics/AbcSize, Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity
  def call(bot_id, notify: true, restart: true, continue_params: nil)
    bot = @bots_repository.find(bot_id)
    return Result::Failure.new unless make_transaction?(bot)

    if continue_params.nil?
      continue_params = { continue_schedule: false, price: nil }
    end

    continue_schedule = continue_params[:continue_schedule]
    fixing_price = continue_params[:price]
    result = continue_schedule ? nil : perform_action(get_api(bot), bot, fixing_price)

    if continue_schedule || result.success?
      bot = @bots_repository.update(bot.id, restarts: 0)
      result = validate_limit(bot, notify)
      check_if_trial_ending_soon(bot, notify) # Send e-mail if ending soon
      @schedule_transaction.call(bot) if result.success?
    elsif restart && recoverable?(result)
      bot = @bots_repository.update(bot.id, restarts: bot.restarts + 1)
      @schedule_transaction.call(bot)
      @notifications.restart_occured(bot: bot, errors: result.errors) if notify
      result = Result::Success.new
    else
      stop_bot(bot, notify, result.errors)
    end

    result
  rescue StandardError
    @unschedule_transactions.call(bot)
    stop_bot(bot, notify)
    raise
  end

  # rubocop:enable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity

  private

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
      force_smart_intervals: fixing_transaction?(price) ? false : bot.force_smart_intervals
    }.compact
    result = if bot.buyer?
               api.buy(settings)
             else
               api.sell(settings)
             end

    @transactions_repository.create(transaction_params(result, bot, price))

    if result.success?
      cost = result.data[:rate].to_f * result.data[:amount].to_f
      @subtract_credits.call(bot, cost)
      bot.reload
    end

    result
  end

  # rubocop:enable Metrics/AbcSize

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

  def check_if_trial_ending_soon(bot, notify)
    ending_soon_result = @validate_trial_ending_soon.call(bot.user)
    @notifications.first_month_ending_soon(bot: bot) if ending_soon_result.failure? && notify
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

  def make_transaction?(bot)
    bot.working?
  end

  def stop_bot(bot, notify, errors = ['Something went wrong!'])
    bot = @bots_repository.update(bot.id, status: 'stopped')
    @notifications.error_occured(bot: bot, errors: errors) if notify
  end
end
# rubocop:enable Metrics/ClassLength
