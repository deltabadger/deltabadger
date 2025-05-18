module Bots::Barbell::OrderSetter # rubocop:disable Metrics/ModuleLength
  extend ActiveSupport::Concern

  include Bots::Barbell::Measurable
  include Bot::Schedulable

  included do
    store_accessor :transient_data,
                   :missed_quote_amount

    validates :missed_quote_amount,
              numericality: { greater_than_or_equal_to: 0 }

    before_save :set_missed_quote_amount, if: :will_save_change_to_settings?
  end

  def missed_quote_amount
    value = super
    value.present? ? value.to_d : 0
  end

  def set_barbell_orders(
    total_orders_amount_in_quote:,
    update_missed_quote_amount: false
  )
    # return Result::Success.new

    raise StandardError, 'quote_amount is required' if total_orders_amount_in_quote.blank?
    raise StandardError, 'quote_amount must be positive' if total_orders_amount_in_quote.negative?
    return Result::Success.new if total_orders_amount_in_quote.zero?

    result = get_barbell_orders(total_orders_amount_in_quote)
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

      result = nil
      with_api_key do
        result = market_buy(
          base_asset_id: order_data[:base_asset].id,
          quote_asset_id: order_data[:quote_asset].id,
          amount: amount_info[:amount],
          amount_type: amount_info[:amount_type]
        )
      end

      if result.success?
        order_id = result.data[:order_id]
        Bot::FetchAndCreateOrderJob.perform_later(self, order_id)
        update!(missed_quote_amount: [0, missed_quote_amount - order_data[:quote_amount]].max) if update_missed_quote_amount
      else
        create_failed_order!(order_data.merge!(error_messages: result.errors))
        return result
      end
    end

    Result::Success.new
  end

  def pending_quote_amount
    return quote_amount if started_at.nil?

    from_start = started_at > settings_changed_at
    start_at = from_start ? started_at : settings_changed_at
    total_quote_amount_invested = transactions.success
                                              .where('created_at >= ?', start_at)
                                              .pluck(:quote_amount)
                                              .sum
    intervals_since_start_at = [0, ((last_interval_checkpoint_at - start_at) / interval_duration).floor].max
    intervals_since_start_at += 1 if from_start

    # puts "intervals_since_start_at: #{intervals_since_start_at}"
    # puts "missed_quote_amount: #{missed_quote_amount}"
    # puts "total_quote_amount_invested: #{total_quote_amount_invested}"
    # puts "quote_amount: #{quote_amount}"
    # puts "result: #{quote_amount * intervals_since_start_at + missed_quote_amount - total_quote_amount_invested}"

    quote_amount * intervals_since_start_at + missed_quote_amount - total_quote_amount_invested
  end

  private

  def set_missed_quote_amount
    quote_amount_bak = quote_amount
    interval_bak = interval
    self.quote_amount = settings_was['quote_amount']
    self.interval = settings_was['interval']
    self.missed_quote_amount = pending_quote_amount
    self.quote_amount = quote_amount_bak
    self.interval = interval_bak
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
end
