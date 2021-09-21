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

  LARGEST_TIMEOUT = 2_147_483_647 # js timeout cannot be larger than 2^32

  def call(bot_id:)
    bot = @bots_repository.find(bot_id)
    if was_first_transaction_failed(bot)
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

    unless had_first_transaction(bot)
      return {
        restartType: 'failed'
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

    number_of_corrected_transactions = num_of_corrected_transactions(bot, interval)

    last_paid_transaction_timestamp = bot.last_transaction.created_at.to_i +
                                      number_of_corrected_transactions * interval

    number_of_transactions = ((now - last_paid_transaction_timestamp) / interval).floor

    last_transaction = bot.last_transaction
    if failed?(bot.last_transaction)
      last_transaction = bot.last_successful_transaction
      number_of_transactions += 1
    end

    number_of_transactions * last_transaction.price.to_f *
      (bot.price.to_f / last_transaction.bot_price)
  end

  def calculate_timeout(next_transaction, now, bot)
    timeout = (next_transaction - now)

    if timeout <= 0
      interval = @parse_interval.call(bot)
      timeout = interval - ((now - bot.last_transaction.created_at.to_i) % interval)
    end

    timeout > 1 && timeout -= 1

    # return in millis
    [timeout * 1000, LARGEST_TIMEOUT].min
  end

  def was_first_transaction_failed(bot)
    return true if bot.last_successful_transaction.nil?

    failed?(bot.last_transaction) &&
      bot.last_successful_transaction.created_at.to_i < bot.settings_changed_at.to_i
  end

  def had_first_transaction(bot)
    return false if bot.last_successful_transaction.nil?

    bot.last_successful_transaction.created_at.to_i >= bot.settings_changed_at.to_i
  end

  def num_of_corrected_transactions(bot, interval)
    return 0 if failed?(bot.any_last_transaction)

    ((bot.any_last_transaction.created_at.to_i -
        bot.last_transaction.created_at.to_i) / interval).floor
  end

  def failed?(transaction)
    transaction.status == 'failure'
  end
end
