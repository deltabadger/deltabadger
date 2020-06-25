class NextBotTransactionAt < BaseService
  def initialize(
    parse_interval: ParseInterval.new,
    calculate_restart_delay: CalculateRestartDelay.new
  )
    @parse_interval = parse_interval
    @calculate_restart_delay = calculate_restart_delay
  end

  def call(bot)
    return nil unless bot.working? && bot.transactions.exists?

    delay = if bot.restarts.zero?
              normal_delay(bot)
            else
              restart_delay(bot)
            end

    delay.since(bot.last_transaction.created_at)
  end

  private

  def normal_delay(bot)
    interval = parse_interval.call(bot)

    [interval - bot.delay, 0].max
  end

  def restart_delay(bot)
    (1..bot.restarts)
      .map { |restarts| calculate_restart_delay(restarts) }
      .sum
  end

  attr_reader :parse_interval, :calculate_restart_delay
end
