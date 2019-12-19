class ParseInterval < BaseService
  Error = StandardError

  INTERVALS = %w[month week day hour minute].freeze

  def call(bot)
    user_interval = calculate_user_interval(bot)
    user_price = bot.price.to_f
    fixed_amount = bot.last_transaction.price.to_f

    (fixed_amount * user_interval) / user_price
  end

  private

  def calculate_user_interval(bot)
    interval = bot.interval
    raise Error, 'Invalid interval' if !INTERVALS.include?(interval)

    1.public_send(interval).seconds
  end
end
