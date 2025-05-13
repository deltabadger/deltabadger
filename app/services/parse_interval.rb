class ParseInterval < BaseService
  Error = StandardError

  def call(bot)
    return 0.seconds if bot.webhook?

    last_transaction = set_last_transaction(bot)
    return 0.seconds if last_transaction.nil?

    user_interval = calculate_user_interval(last_transaction)

    user_price = last_transaction.bot_price.to_f
    fixed_amount = if last_transaction.bot.settings['type'] == 'sell'
                     last_transaction.amount.to_f
                   else
                     (last_transaction.quote_amount || 0.0)
                   end
    (fixed_amount * user_interval) / user_price
  end

  private

  def calculate_user_interval(last_transaction)
    interval = last_transaction.bot_interval
    raise Error, 'Invalid interval' if !Bot::INTERVALS.include?(interval)

    1.public_send(interval).seconds
  end

  def set_last_transaction(bot)
    return nil unless bot.last_transaction.present?
    return bot.last_successful_transaction if bot.last_transaction.failure?

    bot.last_transaction
  end
end
