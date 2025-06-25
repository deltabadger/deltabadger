module Bot::OrderSetter
  extend ActiveSupport::Concern

  private

  def calculate_best_amount_info(order_data)
    ticker = order_data[:ticker]
    case exchange.minimum_amount_logic(side: order_data[:side], order_type: order_data[:order_type])
    when :base_or_quote
      minimum_quote_size_in_base = ticker.minimum_quote_size / order_data[:price]
      amount_type = minimum_quote_size_in_base < ticker.minimum_base_size ? :quote : :base
      amount = amount_type == :base ? order_data[:amount] : order_data[:quote_amount]
      minimum_amount = amount_type == :base ? ticker.minimum_base_size : ticker.minimum_quote_size
    when :base_and_quote
      minimum_quote_size_in_base = ticker.adjusted_amount(
        amount: ticker.minimum_quote_size / order_data[:price],
        amount_type: :base,
        method: :ceil
      )
      minimum_amount = [minimum_quote_size_in_base, ticker.minimum_base_size].max
      amount_type = :base
      amount = order_data[:amount]
    when :base
      minimum_amount = ticker.minimum_base_size
      amount_type = :base
      amount = order_data[:amount]
    end

    {
      amount_type: amount_type,
      amount: amount,
      below_minimum_amount: amount < minimum_amount
    }
  end

  def create_order(order_data, amount_info)
    case order_data[:order_type]
    when :market_order
      market_buy(
        ticker: order_data[:ticker],
        amount: amount_info[:amount],
        amount_type: amount_info[:amount_type]
      )
    when :limit_order
      limit_buy(
        ticker: order_data[:ticker],
        amount: amount_info[:amount],
        amount_type: amount_info[:amount_type],
        price: order_data[:price]
      )
    end
  end
end
