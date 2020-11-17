class ParseInterval < BaseService
  Error = StandardError

  INTERVALS = %w[month week day hour].freeze

  def call(bot)
    user_interval = calculate_user_interval(bot)
    user_price = bot.last_transaction.bot_price.to_f
    fixed_amount = bot.last_transaction.price.to_f

    (fixed_amount * user_interval) / user_price
  end

  private

  def calculate_user_interval(bot)
    interval = bot.last_transaction.bot_interval
    raise Error, 'Invalid interval' if !INTERVALS.include?(interval)

    1.public_send(interval).seconds
  end
end
