module Bots::Barbell::OrderSetter # rubocop:disable Metrics/ModuleLength
  extend ActiveSupport::Concern

  include Bots::Barbell::Measurable
  include Bot::Schedulable

  included do
    store_accessor :transient_data, :pending_quote_amount, :last_pending_quote_amount_calculated_at

    validates :pending_quote_amount,
              numericality: { greater_than_or_equal_to: 0 },
              if: -> { pending_quote_amount.present? }
  end

  def pending_quote_amount
    value = super
    value.present? ? value.to_d : nil
  end

  def last_pending_quote_amount_calculated_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def required_balance_buffer
    quote_amount * (3.days.to_f / 1.public_send(interval))
  end

  def get_barbell_orders(total_orders_amount_in_quote)
    metrics_data = metrics(force: true)

    result0 = exchange.get_ask_price(base_asset_id: base0_asset_id, quote_asset_id: quote_asset_id)
    return result0 unless result0.success?

    result1 = exchange.get_ask_price(base_asset_id: base1_asset_id, quote_asset_id: quote_asset_id)
    return result1 unless result1.success?

    Result::Success.new(calculate_orders_data(
                          balance0: metrics_data[:base0_total_amount],
                          balance1: metrics_data[:base1_total_amount],
                          price0: result0.data,
                          price1: result1.data,
                          total_orders_amount_in_quote: total_orders_amount_in_quote
                        ))
  end

  def set_barbell_orders
    calculate_pending_quote_amount
    return Result::Success.new if pending_quote_amount.zero?

    result = exchange.get_balance(asset_id: quote_asset_id)
    unless result.success?
      create_failed_order!({
                             base_asset: base0_asset,
                             quote_asset: quote_asset,
                             error_messages: result.errors
                           })
      return result
    end

    quote_balance = result.data
    if quote_balance[:free] >= pending_quote_amount && quote_balance[:free] < pending_quote_amount + required_balance_buffer
      notify_end_of_funds
    end

    result = get_barbell_orders(pending_quote_amount)
    unless result.success?
      create_failed_order!({
                             base_asset: base0_asset,
                             quote_asset: quote_asset,
                             error_messages: result.errors
                           })
      return result
    end

    orders_data = result.data
    orders_data.each do |order_data|
      next if order_data[:amount].zero?

      amount_info = calculate_best_amount_info(order_data)
      if amount_info[:below_minimum_amount]
        create_skipped_order!(order_data)
        next
      end

      result = market_buy(
        base_asset_id: order_data[:base_asset].id,
        quote_asset_id: order_data[:quote_asset].id,
        amount: amount_info[:amount],
        amount_type: amount_info[:amount_type]
      )
      if result.success?
        update!(pending_quote_amount: pending_quote_amount - order_data[:quote_amount])
        order_id = result.data[:order_id]
        Bot::FetchAndCreateOrderJob.perform_later(self, order_id)
      else
        create_failed_order!(order_data.merge!(error_messages: result.errors))
        return result
      end
    end

    Result::Success.new
  end

  def calculate_pending_quote_amount
    now = last_interval_checkpoint_at
    last_calc_at = last_pending_quote_amount_calculated_at

    calculated_amount = if last_calc_at.present?
                          intervals_since_last_calc = ((now - last_calc_at) / 1.public_send(interval)).floor
                          missed_quote_amount = quote_amount * intervals_since_last_calc
                          pending_quote_amount + missed_quote_amount
                        else
                          quote_amount
                        end

    update!(
      pending_quote_amount: calculated_amount,
      last_pending_quote_amount_calculated_at: now
    )
  end

  def calculate_best_amount_info(order_data)
    ticker = exchange.tickers.find_by!(base_asset: order_data[:base_asset], quote_asset: order_data[:quote_asset])

    minimum_quote_size_in_base = ticker.minimum_quote_size / order_data[:rate]
    minimum_base_size_in_base = ticker.minimum_base_size
    amount_type = minimum_quote_size_in_base < minimum_base_size_in_base ? :quote : :base
    amount = amount_type == :base ? order_data[:amount] : order_data[:quote_amount]
    minimum_amount_in_base = amount_type == :base ? minimum_base_size_in_base : minimum_quote_size_in_base

    {
      amount_type: amount_type,
      amount: amount,
      below_minimum_amount: amount < minimum_amount_in_base
    }
  end

  private

  def calculate_orders_data(balance0:, balance1:, price0:, price1:, total_orders_amount_in_quote:)
    effective_allocation1 = 1 - effective_allocation0
    balance0_in_quote = balance0 * price0
    balance1_in_quote = balance1 * price1
    total_balance_in_quote = balance0_in_quote + balance1_in_quote + total_orders_amount_in_quote
    target_balance0_in_quote = total_balance_in_quote * effective_allocation0
    target_balance1_in_quote = total_balance_in_quote * effective_allocation1
    base0_offset = [0, target_balance0_in_quote - balance0_in_quote].max
    base1_offset = [0, target_balance1_in_quote - balance1_in_quote].max
    base0_order_size_in_quote = [base0_offset, total_orders_amount_in_quote].min
    base1_order_size_in_quote = [base1_offset, total_orders_amount_in_quote - base0_order_size_in_quote].min
    base0_order_size_in_base = base0_order_size_in_quote / price0
    base1_order_size_in_base = base1_order_size_in_quote / price1
    [
      {
        base_asset: base0_asset,
        quote_asset: quote_asset,
        rate: price0,
        amount: base0_order_size_in_base,
        quote_amount: base0_order_size_in_quote
      },
      {
        base_asset: base1_asset,
        quote_asset: quote_asset,
        rate: price1,
        amount: base1_order_size_in_base,
        quote_amount: base1_order_size_in_quote
      }
    ]
  end
end
