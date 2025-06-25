module Bots::DcaSingleAsset::OrderSetter
  extend ActiveSupport::Concern

  include Bot::OrderSetter

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

    Rails.logger.info(
      "set_order for bot #{id} creating order #{order_data.inspect} " \
      "with amount info #{amount_info.inspect}"
    )
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
end
