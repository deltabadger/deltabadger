module BarbellBot::OrderSetter # rubocop:disable Metrics/ModuleLength
  extend ActiveSupport::Concern

  included do
    store_accessor :transient_data, :pending_quote_amount, :last_pending_quote_amount_calculated_at_iso8601

    validates :pending_quote_amount,
              numericality: { greater_than_or_equal_to: 0 },
              on: :update,
              if: -> { pending_quote_amount.present? }
  end

  def set_barbell_orders
    calculate_pending_quote_amount
    return Result::Success.new if pending_quote_amount.zero?

    result = exchange.get_balances(assets: [quote, base0, base1])
    return result unless result.success?

    balances = result.data
    if balances[quote][:free] < pending_quote_amount
      stop
      # TODO: notify user
      return Result::Failure.new('Insufficient quote balance')
    end

    result = exchange.get_ask_price(base_asset: base0, quote_asset: quote)
    return result unless result.success?

    price0 = result.data
    result = exchange.get_ask_price(base_asset: base1, quote_asset: quote)
    return result unless result.success?

    price1 = result.data
    orders_data = calculate_orders_data(
      balance0: balances[base0][:free],
      balance1: balances[base1][:free],
      price0: price0,
      price1: price1
    )
    orders_data.each do |order_data|
      result = set_order(order_data)
      return result unless result.success?
    end

    Result::Success.new
  end

  def calculate_pending_quote_amount
    now = Time.current

    calculated_amount = if last_pending_quote_amount_calculated_at_iso8601.present?
                          last_calc_at = DateTime.parse(last_pending_quote_amount_calculated_at_iso8601)
                          intervals_since_last_calc = ((now - last_calc_at) / 1.public_send(interval)).floor
                          missed_quote_amount = quote_amount * intervals_since_last_calc
                          pending_quote_amount + missed_quote_amount
                        else
                          quote_amount
                        end

    update!(
      pending_quote_amount: calculated_amount,
      last_pending_quote_amount_calculated_at_iso8601: now.iso8601
    )
  end

  private

  def calculate_orders_data(balance0:, balance1:, price0:, price1:)
    allocation1 = 1 - allocation0
    balance0_in_quote = balance0 * price0
    balance1_in_quote = balance1 * price1
    total_balance_in_quote = balance0_in_quote + balance1_in_quote + quote_amount
    target_balance0_in_quote = total_balance_in_quote * allocation0
    target_balance1_in_quote = total_balance_in_quote * allocation1
    base0_offset = [0, target_balance0_in_quote - balance0_in_quote].max
    base1_offset = [0, target_balance1_in_quote - balance1_in_quote].max
    base0_order_size_in_quote = [base0_offset, quote_amount].min
    base1_order_size_in_quote = [base1_offset, quote_amount - base0_order_size_in_quote].min
    base0_order_size_in_base = base0_order_size_in_quote / price0
    base1_order_size_in_base = base1_order_size_in_quote / price1
    [
      {
        base: base0,
        quote: quote,
        rate: price0,
        amount: base0_order_size_in_base,
        quote_amount: base0_order_size_in_quote
      },
      {
        base: base1,
        quote: quote,
        rate: price1,
        amount: base1_order_size_in_base,
        quote_amount: base1_order_size_in_quote
      }
    ]
  end

  def set_order(order_data)
    result = exchange.get_symbol_info(
      base_asset: order_data[:base],
      quote_asset: order_data[:quote]
    )
    return result unless result.success?

    # TODO: in some cases rate is 0 ?!

    symbol_info = result.data
    return Result::Success.new(create_skipped_order!(order_data)) if order_data[:amount] < symbol_info[:minimum_base_size]

    result = market_buy(
      base_asset: order_data[:base],
      quote_asset: order_data[:quote],
      amount: order_data[:amount],
      amount_type: :base
    )

    if result.success?
      update!(pending_quote_amount: pending_quote_amount - order_data[:quote_amount])

      order_id = Utilities::Hash.dig_or_raise(result.data, 'success_response', 'order_id')
      Bot::CreateSuccessfulOrderJob.perform_later(id, order_id)

      # check_allowable_balance(get_api(bot), bot, fixing_price, notify)
      # send_user_to_sendgrid(bot)
    else
      create_failed_order!(order_data, result.errors)
      # TODO: notify user?
      # TODO: stop the bot?
    end
    result
  end
end
