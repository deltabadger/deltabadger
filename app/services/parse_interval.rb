class ParseInterval < BaseService
  Error = StandardError

  def call(bot)
    return 0.seconds if bot.webhook?

    last_transaction = set_last_transaction(bot)
    return 0.seconds if last_transaction.nil?

    user_interval = calculate_user_interval(last_transaction)

    user_price = last_transaction.bot_quote_amount.to_f
    user_price = bot.price.to_f if user_price.zero?

    fixed_amount = if last_transaction.bot.settings['type'] == 'sell'
                     last_transaction.amount.to_f
                   else
                     (quote_amount(last_transaction) || user_price)
                   end
    fixed_amount = user_price if fixed_amount.zero?

    (fixed_amount * user_interval) / user_price
  end

  private

  def calculate_user_interval(last_transaction)
    interval = last_transaction.bot_interval
    raise Error, 'Invalid interval' if !Bot::Schedulable::INTERVALS.keys.include?(interval)

    1.public_send(interval).seconds
  end

  def set_last_transaction(bot)
    return nil unless bot.last_transaction.present?
    return bot.last_successful_transaction if bot.last_transaction.failed?

    bot.last_transaction
  end

  def quote_amount(transaction)
    return nil unless transaction.amount.present? && transaction.price.present?

    # workaround for unfilled limit orders in legacy bots
    return transaction.bot_quote_amount if transaction.bot.settings['order_type'] == 'limit' &&
                                           (transaction.amount.zero? || transaction.price.zero?)

    transaction.amount * transaction.price
  end
end
