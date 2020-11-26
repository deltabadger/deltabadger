class ParseInterval < BaseService
  Error = StandardError

  INTERVALS = %w[month week day hour].freeze

  def call(bot)
    user_interval = calculate_user_interval(bot)
    last_transaction = bot.last_transaction
    if bot.last_transaction.status == 'failure'
      last_transaction = bot.last_successful_transaction
    end

    user_price = last_transaction.bot_price.to_f
    fixed_amount = last_transaction.price.to_f

    (fixed_amount * user_interval) / user_price
  end

  private

  def calculate_user_interval(bot)
    interval = bot.last_transaction.bot_interval
    raise Error, 'Invalid interval' if !INTERVALS.include?(interval)

    1.public_send(interval).seconds
  end
end
