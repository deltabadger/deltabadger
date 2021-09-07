class ParseInterval < BaseService
  Error = StandardError

  INTERVALS = %w[month week day hour].freeze

  def call(bot)
    last_transaction = set_last_transaction(bot)
    user_interval = calculate_user_interval(last_transaction)

    user_price = last_transaction.bot_price.to_f
    fixed_amount = if last_transaction.bot.settings['type'] == 'sell'
                     last_transaction.amount.to_f
                   else
                     last_transaction.price.to_f
                   end
    (fixed_amount * user_interval) / user_price
  end

  private

  def calculate_user_interval(last_transaction)
    interval = last_transaction.bot_interval
    raise Error, 'Invalid interval' if !INTERVALS.include?(interval)

    1.public_send(interval).seconds
  end

  def set_last_transaction(bot)
    return bot.last_successful_transaction if bot.last_transaction.status == 'failure'

    bot.last_transaction
  end
end
