class ParseInterval < BaseService
  Error = StandardError

  INTERVALS = %w[month week day hour].freeze

  def call(bot)
    last_transaction = bot.last_successful_transaction
    return 0.0 if last_transaction.nil?

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
    return 0.0 if last_transaction.bot.webhook?
    raise Error, 'Invalid interval' if !INTERVALS.include?(interval)

    1.public_send(interval).seconds
  end
end
