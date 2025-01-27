class MakeWithdrawal < BaseService
  def initialize(
    exchange_withdrawal_processor: ExchangeApi::WithdrawalProcessors::Get.new,
    exchange_withdrawal_info_processor: ExchangeApi::WithdrawalInfo::Get.new,
    schedule_withdrawal: ScheduleWithdrawal.new,
    unschedule_transactions: UnscheduleTransactions.new,
    bots_repository: BotsRepository.new,
    transactions_repository: TransactionsRepository.new,
    order_flow_helper: Helpers::OrderFlowHelper.new,
    notifications: Notifications::BotAlerts.new
  )
    @get_withdrawal_processor = exchange_withdrawal_processor
    @get_withdrawal_info_processor = exchange_withdrawal_info_processor
    @schedule_withdrawal = schedule_withdrawal
    @unschedule_transactions = unschedule_transactions
    @bots_repository = bots_repository
    @transactions_repository = transactions_repository
    @order_flow_helper = order_flow_helper
    @notifications = notifications
  end

  SKIPPED = { data: { skipped: true }.freeze }.freeze

  def call(bot_id)
    bot = @bots_repository.find(bot_id)
    return Result::Failure.new unless make_transaction?(bot)

    result = perform_action(bot)
    if result&.success?
      @transactions_repository.create(transaction_params(result, bot))
      bot = @bots_repository.update(bot.id, status: 'working', restarts: 0, account_balance: 0.0)
      @schedule_withdrawal.call(bot)
    elsif insufficient_balance?(result)
      @transactions_repository.create(skipped_transaction_params(bot))
      bot = @bots_repository.update(bot.id, status: 'working', restarts: 0)
      @schedule_withdrawal.call(bot)
    else
      @transactions_repository.create(failed_transaction_params(result, bot))
      Rails.logger.info '=======================  make_withdrawal ELSE =============================='
      Rails.logger.info "================= #{result.errors.inspect} ======================="
      Rails.logger.info '====================================================='
      @order_flow_helper.stop_bot(bot, true, result.errors)
    end
    result
  rescue StandardError
    Rails.logger.info '=======================  make_withdrawal RESCUE 2 =============================='
    Rails.logger.info "================= #{e.inspect} ======================="
    Rails.logger.info '====================================================='
    @unschedule_transactions.call(bot)
    @order_flow_helper.stop_bot(bot, true)
    raise
  end

  private

  def perform_action(bot)
    api_key = ApiKey.for_bot(bot.user_id, bot.exchange_id, 'withdrawal').first
    account_info_api = @get_withdrawal_info_processor.call(api_key)
    withdrawal_api = @get_withdrawal_processor.call(api_key)
    balance = account_info_api.available_funds(bot)
    Rails.logger.info "withdrawal balance: #{balance.inspect}" # TODO: delete after testing
    return balance unless balance.success?

    bot = @bots_repository.update(bot.id, account_balance: balance.data)
    Rails.logger.info "withdrawal bot: #{bot.inspect}" # TODO: delete after testing
    Rails.logger.info "withdrawal threshold : #{check_balance_threshold(bot, balance).inspect}" # TODO: delete after testing
    return Result::Failure.new(**SKIPPED) unless check_balance_threshold(bot, balance)

    Rails.logger.info "withdrawal params: #{get_withdrawal_params(bot, balance).inspect}" # TODO: delete after testing
    withdrawal_api.make_withdrawal(get_withdrawal_params(bot, balance))
  end

  def check_balance_threshold(bot, balance)
    (balance.success? && bot.threshold_enabled && balance.data >= bot.threshold.to_f) || !bot.threshold_enabled
  end

  def insufficient_balance?(result)
    result.data&.dig(:skipped)
  end

  def get_withdrawal_params(bot, balance)
    params = {
      amount: balance.data,
      address: bot.address,
      currency: bot.currency
    } # TODO: delete after testing
    Rails.logger.info "withdrawal params: #{params.inspect}" # TODO: delete after testing
    {
      amount: balance.data,
      address: bot.address,
      currency: bot.currency
    }
  end

  def transaction_params(result, bot)
    result.data.slice(:offer_id, :amount).merge(
      bot_id: bot.id,
      status: :success,
      transaction_type: 'WITHDRAWAL'
    )
  end

  def failed_transaction_params(result, bot)
    {
      bot_id: bot.id,
      status: :failure,
      error_messages: result.errors,
      transaction_type: 'WITHDRAWAL'
    }
  end

  def skipped_transaction_params(bot)
    {
      bot_id: bot.id,
      status: :skipped,
      transaction_type: 'WITHDRAWAL'
    }
  end

  def make_transaction?(bot)
    bot.working? || bot.pending?
  end

  def recoverable?(result)
    result.data&.dig(:recoverable) == true
  end
end
