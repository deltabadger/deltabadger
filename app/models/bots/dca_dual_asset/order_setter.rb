module Bots::DcaDualAsset::OrderSetter
  extend ActiveSupport::Concern

  include Bot::OrderSetter

  def set_orders(
    total_orders_amount_in_quote:,
    update_missed_quote_amount: false
  )
    Rails.logger.info(
      "set_orders for bot #{id} " \
      "with total_orders_amount_in_quote: #{total_orders_amount_in_quote}, " \
      "update_missed_quote_amount: #{update_missed_quote_amount}"
    )
    validate_orders_amount!(total_orders_amount_in_quote)
    return Result::Success.new if total_orders_amount_in_quote.zero?

    result = get_orders_data(total_orders_amount_in_quote)
    return result if result.failure?

    orders_data = result.data
    orders_data.each do |order_data|
      if order_data[:amount].zero?
        Rails.logger.info("set_orders for bot #{id} ignoring order #{order_data.inspect}")
        next
      end

      amount_info = calculate_best_amount_info(order_data)
      if amount_info[:below_minimum_amount]
        Rails.logger.info("set_orders for bot #{id} creating skipped order #{order_data.inspect}")
        create_skipped_order!(order_data)
        next
      end

      Rails.logger.info(
        "set_orders for bot #{id} creating order #{order_data.inspect} " \
        "with amount info #{amount_info.inspect}"
      )
      result = create_order(order_data, amount_info)
      if result.failure?
        Rails.logger.error(
          "set_orders for bot #{id} failed to create order #{order_data.inspect} with amount info #{amount_info.inspect}. " \
          "Errors: #{result.errors.to_sentence}"
        )
        create_failed_order!(order_data.merge!(error_messages: result.errors))
        return result
      else
        order_id = result.data[:order_id]
        Rails.logger.info("set_orders for bot #{id} created order #{order_id} with amount info #{amount_info.inspect}")
        Bot::FetchAndCreateOrderJob.perform_later(
          self,
          order_id,
          update_missed_quote_amount: update_missed_quote_amount
        )
      end
    end

    Result::Success.new
  end

  private

  def validate_orders_amount!(total_orders_amount_in_quote)
    raise 'Orders quote_amount is required' if total_orders_amount_in_quote.blank?
    raise 'Orders quote_amount must be positive' if total_orders_amount_in_quote.negative?
  end

  def get_orders_data(total_orders_amount_in_quote)
    metrics_data = metrics(force: true)

    result0 = if limit_ordered?
                ticker0.get_last_price
              else
                ticker0.get_ask_price
              end
    if result0.failure?
      Rails.logger.error("set_orders for bot #{id} failed to get order0. Errors: #{result0.errors.to_sentence}")
      create_failed_order!(ticker: ticker0, error_messages: result0.errors)
      return result0
    end

    price0 = if limit_ordered?
               ticker0.adjusted_price(price: result0.data * (1 - limit_order_pcnt_distance))
             else
               result0.data
             end

    result1 = if limit_ordered?
                ticker1.get_last_price
              else
                ticker1.get_ask_price
              end
    if result1.failure?
      Rails.logger.error("set_orders for bot #{id} failed to get order1. Errors: #{result1.errors.to_sentence}")
      create_failed_order!(ticker: ticker1, error_messages: result1.errors)
      return result1
    end

    price1 = if limit_ordered?
               ticker1.adjusted_price(price: result1.data * (1 - limit_order_pcnt_distance))
             else
               result1.data
             end

    orders_data = calculate_orders_data(
      balance0: metrics_data[:total_base0_amount],
      balance1: metrics_data[:total_base1_amount],
      price0: price0,
      price1: price1,
      total_orders_amount_in_quote: total_orders_amount_in_quote,
      order_type: limit_ordered? ? :limit_order : :market_order
    )
    Result::Success.new(orders_data)
  end

  def calculate_orders_data(balance0:, balance1:, price0:, price1:, total_orders_amount_in_quote:, order_type:)
    allocation1 = 1 - allocation0
    balance0_in_quote = balance0 * price0
    balance1_in_quote = balance1 * price1
    total_balance_in_quote = balance0_in_quote + balance1_in_quote + total_orders_amount_in_quote
    target_balance0_in_quote = total_balance_in_quote * allocation0
    target_balance1_in_quote = total_balance_in_quote * allocation1
    base0_offset = [0, target_balance0_in_quote - balance0_in_quote].max
    base1_offset = [0, target_balance1_in_quote - balance1_in_quote].max
    base0_order_size_in_quote = [base0_offset, total_orders_amount_in_quote].min
    base1_order_size_in_quote = [base1_offset, total_orders_amount_in_quote - base0_order_size_in_quote].min
    base0_order_size_in_base = base0_order_size_in_quote / price0
    base1_order_size_in_base = base1_order_size_in_quote / price1
    [
      {
        ticker: ticker0,
        price: price0,
        amount: base0_order_size_in_base,
        quote_amount: base0_order_size_in_quote,
        side: :buy,
        order_type: order_type
      },
      {
        ticker: ticker1,
        price: price1,
        amount: base1_order_size_in_base,
        quote_amount: base1_order_size_in_quote,
        side: :buy,
        order_type: order_type
      }
    ]
  end
end
