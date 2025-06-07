module Bots::DcaDualAsset::OrderSetter # rubocop:disable Metrics/ModuleLength
  extend ActiveSupport::Concern

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

    result = get_orders(total_orders_amount_in_quote)
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

      result = create_order(order_data, amount_info)
      if result.failure?
        Rails.logger.error(
          "set_orders for bot #{id} failed to create order #{order_data.inspect}. " \
          "Errors: #{result.errors.to_sentence}"
        )
        create_failed_order!(order_data.merge!(error_messages: result.errors))
        return result
      else
        order_id = result.data[:order_id]
        Rails.logger.info("set_orders for bot #{id} created order #{order_id}")
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

  def get_orders(total_orders_amount_in_quote)
    metrics_data = metrics(force: true)

    result0 = ticker0.get_ask_price
    if result0.failure?
      Rails.logger.error("set_orders for bot #{id} failed to get order0. Errors: #{result0.errors.to_sentence}")
      create_failed_order!(ticker: ticker0, error_messages: result0.errors)
      return result0
    end

    result1 = ticker1.get_ask_price
    if result1.failure?
      Rails.logger.error("set_orders for bot #{id} failed to get order1. Errors: #{result1.errors.to_sentence}")
      create_failed_order!(ticker: ticker1, error_messages: result1.errors)
      return result1
    end

    orders_data = calculate_orders_data(
      balance0: metrics_data[:total_base0_amount],
      balance1: metrics_data[:total_base1_amount],
      price0: result0.data,
      price1: result1.data,
      total_orders_amount_in_quote: total_orders_amount_in_quote
    )
    Result::Success.new(orders_data)
  end

  def calculate_orders_data(balance0:, balance1:, price0:, price1:, total_orders_amount_in_quote:)
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
        quote_amount: base0_order_size_in_quote
      },
      {
        ticker: ticker1,
        price: price1,
        amount: base1_order_size_in_base,
        quote_amount: base1_order_size_in_quote
      }
    ]
  end

  def calculate_best_amount_info(order_data)
    ticker = order_data[:ticker]
    case exchange.minimum_amount_logic
    when :base_or_quote
      minimum_quote_size_in_base = ticker.minimum_quote_size / order_data[:price]
      amount_type = minimum_quote_size_in_base < ticker.minimum_base_size ? :quote : :base
      amount = amount_type == :base ? order_data[:amount] : order_data[:quote_amount]
      minimum_amount = amount_type == :base ? ticker.minimum_base_size : ticker.minimum_quote_size
    when :base_and_quote
      minimum_amount = [ticker.minimum_quote_size / order_data[:price], ticker.minimum_base_size].max
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
      "set_orders for bot #{id} creating order #{order_data.inspect} " \
      "with amount info #{amount_info.inspect}"
    )
    with_api_key do
      order_data[:ticker].market_buy(
        amount: amount_info[:amount],
        amount_type: amount_info[:amount_type]
      )
    end
  end
end
