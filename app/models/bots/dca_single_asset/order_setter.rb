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
      Rails.logger.info("set_order bot=#{id} event=order_ignored #{order_log_fields(order_data)}")
      log_activity('order_ignored', details: order_log_details(order_data))
      return Result::Success.new
    end

    amount_info = calculate_best_amount_info(order_data)
    if amount_info[:below_minimum_amount]
      Rails.logger.info("set_order bot=#{id} event=order_skipped #{order_log_fields(order_data)}")
      log_activity('order_skipped', level: :warning, details: order_log_details(order_data))
      create_skipped_order!(order_data)
      return Result::Success.new
    end

    Rails.logger.info("set_order bot=#{id} event=order_creating #{order_log_fields(order_data)}")
    result = create_order(order_data, amount_info)
    if result.failure?
      Rails.logger.error(
        "set_order bot=#{id} event=order_failed #{order_log_fields(order_data)} " \
        "errors=#{result.errors.to_sentence}"
      )
      # A -1021/timestamp rejection is a no-op pre-trade rejection: no order was placed, so don't
      # leave a misleading `failed` Transaction row. The bot reschedules cleanly (Bot::ActionJob).
      create_failed_order!(order_data.merge!(error_messages: result.errors)) unless exchange.placement_transient_error?(result.errors)
      return result
    else
      order_id = result.data[:order_id]
      Rails.logger.info("set_order bot=#{id} event=order_accepted order_id=#{order_id} #{order_log_fields(order_data)}")
      transaction = persist_accepted_order!(order_data, order_id)
      Bot::FetchAndUpdateOrderJob.perform_later(
        transaction,
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
              ticker.adjusted_price(price: result.data * (1.to_d - limit_order_pcnt_distance_decimal))
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
