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

  def set_barbell_orders
    calculate_pending_quote_amount
    return Result::Success.new if pending_quote_amount.zero?

    puts 'Setting barbell orders'

    result = exchange.get_balance(asset_id: quote_asset_id)
    return result unless result.success?

    puts "Quote balance: #{result.data}"

    quote_balance = result.data
    notify_end_of_funds if quote_balance[:free] < pending_quote_amount + quote_amount
    if quote_balance[:free] < pending_quote_amount
      stop
      return Result::Success.new
    end

    result = get_metrics_with_current_prices
    return result unless result.success?

    result0 = exchange.get_ask_price(base_asset_id: base0_asset_id, quote_asset_id: quote_asset_id)
    return result0 unless result0.success?

    result1 = exchange.get_ask_price(base_asset_id: base1_asset_id, quote_asset_id: quote_asset_id)
    return result1 unless result1.success?

    puts 'Calculating orders data'
    orders_data = calculate_orders_data(
      balance0: result.data[:total_base0_amount_acquired],
      balance1: result.data[:total_base1_amount_acquired],
      price0: result0.data,
      price1: result1.data
    )
    puts "Orders data: #{orders_data}"
    orders_data.each do |order_data|
      puts "Setting order: #{order_data}"
      result = set_order(order_data)
      return result unless result.success?
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

  private

  def calculate_orders_data(balance0:, balance1:, price0:, price1:)
    allocation1 = 1 - allocation0
    balance0_in_quote = balance0 * price0
    balance1_in_quote = balance1 * price1
    total_balance_in_quote = balance0_in_quote + balance1_in_quote + pending_quote_amount
    target_balance0_in_quote = total_balance_in_quote * allocation0
    target_balance1_in_quote = total_balance_in_quote * allocation1
    base0_offset = [0, target_balance0_in_quote - balance0_in_quote].max
    base1_offset = [0, target_balance1_in_quote - balance1_in_quote].max
    base0_order_size_in_quote = [base0_offset, pending_quote_amount].min
    base1_order_size_in_quote = [base1_offset, pending_quote_amount - base0_order_size_in_quote].min
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

  def set_order(order_data)
    ticker = exchange.tickers.find_by!(base_asset: order_data[:base_asset], quote_asset: order_data[:quote_asset])

    minimum_quote_size_in_base = ticker.minimum_quote_size / order_data[:rate]
    minimum_base_size_in_base = ticker.minimum_base_size
    amount_type = minimum_quote_size_in_base < minimum_base_size_in_base ? :quote : :base
    minimum_amount = amount_type == :base ? minimum_base_size_in_base : minimum_quote_size_in_base

    return Result::Success.new(create_skipped_order!(order_data)) if order_data[:amount] < minimum_amount

    result = market_buy(
      base_asset_id: order_data[:base_asset].id,
      quote_asset_id: order_data[:quote_asset].id,
      amount: amount_type == :base ? order_data[:amount] : order_data[:quote_amount],
      amount_type: amount_type
    )

    if result.success?
      update!(pending_quote_amount: pending_quote_amount - order_data[:quote_amount])

      order_id = result.data[:order_id]
      Bot::CreateSuccessfulOrderJob.perform_later(self, order_id)
      # send_user_to_sendgrid(bot)
    else
      order_data.merge!(error_messages: result.errors)
      create_failed_order!(order_data)
      notify_about_error(errors: result.errors)
      # TODO: stop the bot?
    end
    result
  end
end
