module Bots::DcaIndex::OrderSetter
  extend ActiveSupport::Concern

  include Bot::OrderSetter

  def set_orders(
    total_orders_amount_in_quote:,
    update_missed_quote_amount: false
  )
    Rails.logger.info(
      "set_orders for index bot #{id} " \
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
        Rails.logger.info("set_orders for index bot #{id} ignoring order #{order_data.inspect}")
        next
      end

      amount_info = calculate_best_amount_info(order_data)
      if amount_info[:below_minimum_amount]
        Rails.logger.info("set_orders for index bot #{id} creating skipped order #{order_data.inspect}")
        create_skipped_order!(order_data)
        next
      end

      Rails.logger.info(
        "set_orders for index bot #{id} creating order #{order_data.inspect} " \
        "with amount info #{amount_info.inspect}"
      )
      result = create_order(order_data, amount_info)
      if result.failure?
        Rails.logger.error(
          "set_orders for index bot #{id} failed to create order #{order_data.inspect} with amount info #{amount_info.inspect}. " \
          "Errors: #{result.errors.to_sentence}"
        )
        create_failed_order!(order_data.merge!(error_messages: result.errors))
        return result
      else
        order_id = result.data[:order_id]
        Rails.logger.info("set_orders for index bot #{id} created order #{order_id} with amount info #{amount_info.inspect}")
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
    allocations = current_allocations
    return Result::Failure.new('No assets in index') if allocations.empty?

    metrics_data = metrics(force: true)
    asset_breakdown = metrics_data[:asset_breakdown] || {}

    # Step 1: Get current prices for all assets
    asset_prices = {}
    allocations.each do |alloc|
      ticker = alloc[:ticker]
      next unless ticker.present?

      price_result = if limit_ordered?
                       ticker.get_last_price
                     else
                       ticker.get_ask_price
                     end

      if price_result.failure?
        Rails.logger.error("set_orders for index bot #{id} failed to get price for #{alloc[:symbol]}. Errors: #{price_result.errors.to_sentence}")
        create_failed_order!(ticker: ticker, error_messages: price_result.errors)
        return price_result
      end

      price = if limit_ordered?
                ticker.adjusted_price(price: price_result.data * (1 - limit_order_pcnt_distance))
              else
                price_result.data
              end

      asset_prices[alloc[:symbol]] = { price: price, ticker: ticker, target_allocation: alloc[:target_allocation].to_f }
    end

    # Step 2: Calculate current portfolio value and per-asset values
    current_values = {}
    total_current_value = 0
    asset_prices.each do |symbol, data|
      current_amount = asset_breakdown.dig(symbol, :amount) || 0
      current_value = current_amount * data[:price]
      current_values[symbol] = current_value
      total_current_value += current_value
    end

    # Step 3: Calculate target values after adding new investment
    total_portfolio_value = total_current_value + total_orders_amount_in_quote
    target_values = {}
    asset_prices.each do |symbol, data|
      target_values[symbol] = total_portfolio_value * data[:target_allocation]
    end

    # Step 4: Calculate how much each asset is underweight (offset)
    offsets = {}
    total_offset = 0
    asset_prices.each do |symbol, _data|
      offset = [0, target_values[symbol] - current_values[symbol]].max
      offsets[symbol] = offset
      total_offset += offset
    end

    # Step 5: Distribute investment proportionally among underweight assets
    orders_data = []
    remaining_investment = total_orders_amount_in_quote

    asset_prices.each do |symbol, data|
      offset = offsets[symbol]
      next if offset.zero?

      # Allocate proportionally to offset, capped by remaining investment
      if total_offset > 0
        order_amount_in_quote = [offset, remaining_investment * (offset / total_offset)].min
        order_amount_in_quote = [order_amount_in_quote, remaining_investment].min
      else
        order_amount_in_quote = 0
      end

      next if order_amount_in_quote <= 0

      order_amount_in_base = order_amount_in_quote / data[:price]
      remaining_investment -= order_amount_in_quote

      orders_data << {
        ticker: data[:ticker],
        price: data[:price],
        amount: order_amount_in_base,
        quote_amount: order_amount_in_quote,
        side: :buy,
        order_type: limit_ordered? ? :limit_order : :market_order
      }

      Rails.logger.info(
        "Index bot #{id} rebalance: #{symbol} current=#{current_values[symbol].round(2)}, " \
        "target=#{target_values[symbol].round(2)}, offset=#{offset.round(2)}, " \
        "order=#{order_amount_in_quote.round(2)}"
      )
    end

    Result::Success.new(orders_data)
  end
end
