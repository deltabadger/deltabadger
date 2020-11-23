class GetRestartParams < BaseService
  def initialize(
    parse_interval: ParseInterval.new,
    next_bot_transaction_at: NextBotTransactionAt.new,
    bots_repository: BotsRepository.new
  )
    @parse_interval = parse_interval
    @next_bot_transaction_at = next_bot_transaction_at
    @bots_repository = bots_repository
  end

  def call(bot_id:)
    bot = @bots_repository.find(bot_id)

    if was_last_regular_failed(bot)
      return {
        restartType: 'failed'
      }
    end

    now_timestamp = Time.now.to_i
    next_transaction_timestamp = @next_bot_transaction_at.call(bot).to_i

    if now_timestamp < next_transaction_timestamp
      return {
        restartType: 'onSchedule',
        timeToNextTransaction: next_transaction_timestamp - now_timestamp,
        timeout: calculate_timeout(next_transaction_timestamp, now_timestamp, bot)
      }
    end

    {
      restartType: 'missed',
      missedAmount: calculate_missed_amount(now_timestamp, bot),
      timeout: calculate_timeout(next_transaction_timestamp, now_timestamp, bot)
    }
  end

  private

  def calculate_missed_amount(now, bot)
    interval = @parse_interval.call(bot)
    number_of_transactions = ((now - bot.last_transaction.created_at.to_i) / interval).floor

    number_of_transactions * bot.last_transaction.price.to_f *
      (bot.price.to_f / bot.last_transaction.bot_price.to_f)
  end

  def calculate_timeout(next_transaction, now, bot)
    timeout = (next_transaction - now)

    if timeout <= 0
      interval = @parse_interval.call(bot)
      timeout = interval - ((now - bot.last_transaction.created_at.to_i) % interval)
    end

    timeout > 1 && timeout -= 1

    # return in millis
    timeout * 1000
  end

  def was_last_regular_failed(bot)
    bot.last_transaction.status == 'failure'
  end
end
