class NextTradingBotTransactionAt < BaseService
  def initialize(
    parse_interval: ParseInterval.new,
    calculate_restart_delay: CalculateRestartDelay.new
  )
    @parse_interval = parse_interval
    @calculate_restart_delay = calculate_restart_delay
  end

  def call(bot, first_transaction: false)
    return DateTime.now if first_transaction
    return nil unless bot.transactions.exists?
    return bot.last_transaction.created_at if manual_restart_failed_bot?(bot)

    delay = if bot.restarts.zero?
              normal_delay(bot)
            else
              restart_delay(bot)
            end

    delay.since(last_paid_transaction(bot))
  end

  private

  def manual_restart_failed_bot?(bot)
    bot.any_last_transaction.status == 'failure' && !bot.working? && !bot.pending?
  end

  def normal_delay(bot)
    interval = parse_interval.call(bot)

    [interval - bot.delay, 0.seconds].max
  end

  def restart_delay(bot)
    calculate_restart_delay.call(bot.restarts)
  end

  def last_paid_transaction(bot)
    interval = parse_interval.call(bot)

    number_of_transactions = ((bot.any_last_transaction.created_at -
      bot.last_transaction.created_at) / interval).floor

    bot.last_transaction.created_at + number_of_transactions * interval
  end

  attr_reader :parse_interval, :calculate_restart_delay
end
