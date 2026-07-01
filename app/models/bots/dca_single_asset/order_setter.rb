module Bots::DcaSingleAsset::OrderSetter
  extend ActiveSupport::Concern

  include Bot::OrderSetter

  def set_order(
    order_amount_in_quote: nil,
    side: :buy,
    update_missed_quote_amount: false
  )
    Rails.logger.info(
      "set_order for bot #{id} side=#{side} " \
      "order_amount_in_quote=#{order_amount_in_quote} " \
      "update_missed_quote_amount=#{update_missed_quote_amount}"
    )

    if side == :sell
      sellable = sellable_base_amount
      if sellable.zero?
        # Genuinely nothing to sell (a balance-read FAILURE raises instead of reaching here). Log the
        # reason so a permanently-idle selling bot is greppable in healthcheck-logs.
        Rails.logger.info("set_order bot=#{id} event=sell_skipped reason=#{sell_skip_reason}")
        return Result::Success.new # nothing to sell yet — skip, keep running
      end

      result = get_order_data(side: :sell, base_amount: sellable)
    else
      validate_order_amount!(order_amount_in_quote)
      return Result::Success.new if order_amount_in_quote.zero?

      result = get_order_data(side: :buy, order_amount_in_quote: order_amount_in_quote)
    end
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

  def get_order_data(side:, order_amount_in_quote: nil, base_amount: nil)
    result = reference_price(side)
    if result.failure?
      # A transient/throttle price read must retry via Bot::ActionJob's typed-error chain, not leave
      # a permanent failed Transaction (mirror of the fetch jobs). Any other failure keeps the
      # existing failed-order record.
      raise Client::RateLimitedError, result.errors.to_sentence if exchange.throttled_error?(result.errors)
      raise Client::TransientNetworkError, result.errors.to_sentence if exchange.transient_error?(result.errors)

      Rails.logger.error("set_order for bot #{id} failed to get order. Errors: #{result.errors.to_sentence}")
      create_failed_order!(ticker: ticker, error_messages: result.errors)
      return result
    end

    order_data = calculate_order_data(
      side: side,
      price: order_price(side, result.data),
      order_amount_in_quote: order_amount_in_quote,
      base_amount: base_amount,
      order_type: limit_ordered? ? :limit_order : :market_order
    )
    Result::Success.new(order_data)
  end

  # Market price tracks the side of the spread we cross: buys take the ask, sells take the bid.
  # Limit orders price off the last trade and adjust by limit distance (below for buys, above
  # for sells), so both fetch get_last_price.
  def reference_price(side)
    return ticker.get_last_price if limit_ordered?

    side == :sell ? ticker.get_bid_price : ticker.get_ask_price
  end

  def order_price(side, raw_price)
    return raw_price unless limit_ordered?

    distance = limit_order_pcnt_distance_decimal
    multiplier = side == :sell ? (1.to_d + distance) : (1.to_d - distance)
    ticker.adjusted_price(price: raw_price * multiplier)
  end

  def calculate_order_data(side:, price:, order_type:, order_amount_in_quote: nil, base_amount: nil)
    if side == :sell
      order_size_in_base = base_amount
      quote_amount = base_amount * price
    else
      order_size_in_base = order_amount_in_quote / price
      quote_amount = order_amount_in_quote
    end

    {
      ticker: ticker,
      price: price,
      amount: order_size_in_base,
      quote_amount: quote_amount,
      side: side,
      order_type: order_type
    }
  end

  # The bot only sells what it accumulated and only as much as is actually free on the exchange:
  # min(per-tick desired, net executed holdings, live free base balance, remaining cap allowance).
  # Below the exchange minimum it flows through the existing below-minimum skip path. Never oversell.
  def sellable_base_amount
    desired = effective_base_amount
    return 0.to_d if desired <= 0

    # A selling bot may liquidate the WHOLE wallet, not just what it accumulated, so net holdings
    # (total_amount) is no longer a cap. Cheap DB-only ceilings first (the configured amount and the
    # optional "don't sell more than N" cap); only hit the exchange for the live balance when there is
    # genuinely something to sell, so an unconfigured-amount / cap-exhausted tick makes no balance call.
    ceiling = [desired]
    ceiling << base_amount_available_before_limit_reached if base_amount_limited?
    cap = ceiling.min
    return 0.to_d if cap <= 0

    [cap, live_free_base_balance].min
  end

  # The per-tick sell size: while Smart Intervals is on, the base split; otherwise the full
  # configured sell amount. (Quote-side splitting is the buy-only effective_quote_amount.)
  def effective_base_amount
    return smart_interval_base_amount.to_d if selling? && smart_intervaled? && smart_interval_base_amount.present?

    sell_amount || 0
  end

  # The live free base balance. A failed read must NOT be coerced to 0 — that would make a transient
  # API hiccup look like "nothing to sell" and silently skip the tick. Surface it: transient/throttle
  # errors raise the typed errors Bot::ActionJob retries on; any other failure raises so ActionJob
  # records execution_failed + notifies. Only a genuine free balance of 0 means nothing to sell.
  def live_free_base_balance
    result = get_balance(asset_id: base_asset_id)
    if result.failure?
      raise Client::RateLimitedError, result.errors.to_sentence if exchange.throttled_error?(result.errors)
      raise Client::TransientNetworkError, result.errors.to_sentence if exchange.transient_error?(result.errors)

      raise "Failed to read #{base_asset&.symbol} balance for bot #{id}: #{result.errors.to_sentence}"
    end

    result.data[:free].to_d
  end

  # Why a sell tick found nothing to sell — for healthcheck-logs observability (NOT a balance failure,
  # which raises before we get here).
  def sell_skip_reason
    return 'unconfigured_sell_amount' if effective_base_amount <= 0
    return 'cap_reached' if base_amount_limited? && base_amount_available_before_limit_reached <= 0

    'no_holdings'
  end
end
