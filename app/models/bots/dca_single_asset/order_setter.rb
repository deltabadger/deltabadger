module Bots::DcaSingleAsset::OrderSetter
  extend ActiveSupport::Concern

  def set_order(
    order_amount_in_quote:,
    update_missed_quote_amount: false
  )
    Rails.logger.info(
      "set_order for bot #{id} " \
      "with order_amount_in_quote: #{order_amount_in_quote}, " \
      "update_missed_quote_amount: #{update_missed_quote_amount}"
    )
    validate_order_amount!(order_amount_in_quote)
    return Result::Success.new if order_amount_in_quote.zero?

    result = get_order_data(order_amount_in_quote)
    return result if result.failure?

    order_data = result.data
    if order_data[:amount].zero?
      Rails.logger.info("set_order for bot #{id} ignoring order #{order_data.inspect}")
      return Result::Success.new
    end

    amount_info = calculate_best_amount_info(order_data)
    if amount_info[:below_minimum_amount]
      Rails.logger.info("set_order for bot #{id} creating skipped order #{order_data.inspect}")
      create_skipped_order!(order_data)
      return Result::Success.new
    end

    result = create_order(order_data, amount_info)
    if result.failure?
      Rails.logger.error(
        "set_order for bot #{id} failed to create order #{order_data.inspect} with amount info #{amount_info.inspect}. " \
        "Errors: #{result.errors.to_sentence}"
      )
      create_failed_order!(order_data.merge!(error_messages: result.errors))
      return result
    else
      order_id = result.data[:order_id]
      Rails.logger.info("set_order for bot #{id} created order #{order_id} with amount info #{amount_info.inspect}")
      Bot::FetchAndCreateOrderJob.perform_later(
        self,
        order_id,
        update_missed_quote_amount: update_missed_quote_amount
      )
    end

    Result::Success.new
  end

  private

  def validate_order_amount!(order_amount_in_quote)
    raise 'Order quote_amount is required' if order_amount_in_quote.blank?
    raise 'Order quote_amount must be positive' if order_amount_in_quote.negative?
  end

  def get_order_data(order_amount_in_quote)
    result = if limit_ordered?
               ticker.get_last_price
             else
               ticker.get_ask_price
             end
    if result.failure?
      Rails.logger.error("set_order for bot #{id} failed to get order. Errors: #{result.errors.to_sentence}")
      create_failed_order!(ticker: ticker, error_messages: result.errors)
      return result
    end

    price = if limit_ordered?
              ticker.adjusted_price(price: result.data * (1 - limit_order_pcnt_distance))
            else
              result.data
            end

    order_data = calculate_order_data(
      price: price,
      order_amount_in_quote: order_amount_in_quote,
      order_type: limit_ordered? ? :limit_order : :market_order
    )
    Result::Success.new(order_data)
  end

  def calculate_order_data(price:, order_amount_in_quote:, order_type:)
    order_size_in_base = order_amount_in_quote / price
    {
      ticker: ticker,
      price: price,
      amount: order_size_in_base,
      quote_amount: order_amount_in_quote,
      side: :buy,
      order_type: order_type
    }
  end

  def calculate_best_amount_info(order_data)
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
    Rails.logger.info(
      "set_order for bot #{id} creating order #{order_data.inspect} " \
      "with amount info #{amount_info.inspect}"
    )
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
