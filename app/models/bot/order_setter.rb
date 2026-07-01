module Bot::OrderSetter
  extend ActiveSupport::Concern

  private

  # Stable, aggregatable fields for an order — used for both log lines and the
  # bot_activity_logs details payload (avoids dumping the whole order_data object).
  def order_log_details(order_data)
    ticker = order_data[:ticker]
    {
      base: ticker&.base_asset&.symbol,
      quote: ticker&.quote_asset&.symbol,
      amount: order_data[:amount],
      quote_amount: order_data[:quote_amount],
      price: order_data[:price]
    }
  end

  def order_log_fields(order_data)
    order_log_details(order_data).map { |k, v| "#{k}=#{v}" }.join(' ')
  end

  def calculate_best_amount_info(order_data)
    ticker = order_data[:ticker]
    # Sells always submit a base amount (we size in base), but must still honor the exchange's
    # quote/notional floor — expressed as its base-equivalent. That is exactly the
    # :base_and_quote_in_base logic, so force it for sells regardless of the exchange's buy logic
    # (never bypass quote minimums on :quote / :base_and_quote venues).
    amount_logic = if order_data[:side] == :sell
                     :base_and_quote_in_base
                   else
                     exchange.minimum_amount_logic(side: order_data[:side], order_type: order_data[:order_type])
                   end
    case amount_logic
    when :base_or_quote, :base_and_quote
      minimum_base_size_in_quote = ticker.adjusted_amount(
        amount: ticker.minimum_base_size * order_data[:price],
        amount_type: :quote,
        method: :ceil
      )
      minimum_quote_amount = [minimum_base_size_in_quote, ticker.minimum_quote_size].max
      minimum_quote_amount_in_base = minimum_quote_amount / order_data[:price]
      minimum_quote_size_in_base = ticker.adjusted_amount(
        amount: ticker.minimum_quote_size / order_data[:price],
        amount_type: :base,
        method: :ceil
      )
      minimum_base_amount = [minimum_quote_size_in_base, ticker.minimum_base_size].max
      amount_type = minimum_quote_amount_in_base < minimum_base_amount ? :quote : :base
      amount = amount_type == :base ? order_data[:amount] : order_data[:quote_amount]
      minimum_amount = amount_type == :base ? minimum_base_amount : minimum_quote_amount
    when :base_and_quote_in_base
      minimum_quote_size_in_base = ticker.adjusted_amount(
        amount: ticker.minimum_quote_size / order_data[:price],
        amount_type: :base,
        method: :ceil
      )
      minimum_amount = [minimum_quote_size_in_base, ticker.minimum_base_size].max
      amount_type = :base
      amount = order_data[:amount]
    when :quote
      minimum_amount = ticker.minimum_quote_size
      amount_type = :quote
      amount = order_data[:quote_amount]
    when :base
      minimum_amount = ticker.minimum_base_size
      amount_type = :base
      amount = order_data[:amount]
    end

    adjusted_amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)

    {
      amount_type: amount_type,
      amount: amount,
      below_minimum_amount: adjusted_amount < minimum_amount
    }
  end

  def create_order(order_data, amount_info)
    sell = order_data[:side] == :sell
    case order_data[:order_type]
    when :market_order
      if sell
        market_sell(ticker: order_data[:ticker], amount: amount_info[:amount], amount_type: amount_info[:amount_type])
      else
        market_buy(ticker: order_data[:ticker], amount: amount_info[:amount], amount_type: amount_info[:amount_type])
      end
    when :limit_order
      if sell
        limit_sell(ticker: order_data[:ticker], amount: amount_info[:amount],
                   amount_type: amount_info[:amount_type], price: order_data[:price])
      else
        limit_buy(ticker: order_data[:ticker], amount: amount_info[:amount],
                  amount_type: amount_info[:amount_type], price: order_data[:price])
      end
    end
  end
end
