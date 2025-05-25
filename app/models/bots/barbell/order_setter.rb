module Bots::Barbell::OrderSetter # rubocop:disable Metrics/ModuleLength
  extend ActiveSupport::Concern

  included do
    store_accessor :transient_data,
                   :missed_quote_amount,
                   :missed_quote_amount_was_set

    validates :missed_quote_amount, numericality: { greater_than_or_equal_to: 0 }
    validate :validate_missed_quote_amount_was_set, if: :will_save_change_to_settings?
  end

  def missed_quote_amount
    value = super
    value.present? ? value.to_d : 0
  end

  def set_barbell_orders(
    total_orders_amount_in_quote:,
    update_missed_quote_amount: false
  )
    Rails.logger.info("set_barbell_orders for bot #{id} with total_orders_amount_in_quote: #{total_orders_amount_in_quote}, update_missed_quote_amount: #{update_missed_quote_amount}")
    raise StandardError, 'quote_amount is required' if total_orders_amount_in_quote.blank?
    raise StandardError, 'quote_amount must be positive' if total_orders_amount_in_quote.negative?
    return Result::Success.new if total_orders_amount_in_quote.zero? || total_orders_amount_in_quote.negative?

    result = get_barbell_orders(total_orders_amount_in_quote)
    unless result.success?
      Rails.logger.error("set_barbell_orders for bot #{id} failed to get barbell orders: #{result.errors.inspect}")
      create_failed_order!({
                             base_asset: base0_asset,
                             quote_asset: quote_asset,
                             error_messages: result.errors
                           })
      return result
    end

    orders_data = result.data
    Rails.logger.info("set_barbell_orders for bot #{id} got orders_data: #{orders_data.inspect}")
    orders_data.each do |order_data|
      if order_data[:amount].zero?
        Rails.logger.info("set_barbell_orders for bot #{id} ignoring order #{order_data.inspect}")
        next
      end

      amount_info = calculate_best_amount_info(order_data)
      if amount_info[:below_minimum_amount]
        Rails.logger.info("set_barbell_orders for bot #{id} creating skipped order #{order_data.inspect}")
        create_skipped_order!(order_data)
        next
      end

      Rails.logger.info("set_barbell_orders for bot #{id} creating order #{order_data.inspect} with amount info #{amount_info.inspect}")

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
        Rails.logger.info("set_barbell_orders for bot #{id} created order #{order_id}")
        Bot::FetchAndCreateOrderJob.perform_later(self, order_id)
        update!(missed_quote_amount: [0, missed_quote_amount - order_data[:quote_amount]].max) if update_missed_quote_amount
      else
        Rails.logger.error("set_barbell_orders for bot #{id} failed to create order #{order_data.inspect}: #{result.errors.inspect}")
        create_failed_order!(order_data.merge!(error_messages: result.errors))
        return result
      end
    end

    Result::Success.new
  end

  def pending_quote_amount
    return 0 if started_at.nil? || deleted?

    calc_since = [started_at, settings_changed_at].compact.max
    total_quote_amount_invested = transactions.success
                                              .where('created_at >= ?', calc_since)
                                              .pluck(:quote_amount)
                                              .sum

    intervals = [0, ((last_interval_checkpoint_at - calc_since) / interval_duration).floor].max + 1

    # puts "intervals: #{intervals}"
    # puts "last_interval_checkpoint_at: #{last_interval_checkpoint_at}"
    # puts "started_at:                  #{started_at}"
    # puts "settings_changed_at:         #{settings_changed_at}"
    # puts "calc_since:                  #{calc_since}"
    # puts "current_time:                #{Time.current}"
    # puts "intervals since started_at: #{[0, ((last_interval_checkpoint_at - started_at) / interval_duration).floor].max + 1}"
    # puts "intervals since settings_changed_at: #{[0,
    #                                               ((last_interval_checkpoint_at - settings_changed_at) / interval_duration).floor].max + 1}"
    # puts "interval_duration: #{interval_duration}"
    # puts "missed_quote_amount: #{missed_quote_amount}"
    # puts "total_quote_amount_invested: #{total_quote_amount_invested}"
    # puts "quote_amount: #{quote_amount}"
    # puts "normal_interval_quote_amount: #{normal_interval_quote_amount}"
    # puts "interval: #{interval}"
    # puts "interval_duration: #{interval_duration}"
    # puts "normal_interval_duration: #{normal_interval_duration}"
    # puts "result: #{quote_amount * intervals + missed_quote_amount - total_quote_amount_invested}"

    [quote_amount * intervals + missed_quote_amount - total_quote_amount_invested, 0].max
  end

  def set_missed_quote_amount
    self.missed_quote_amount = pending_quote_amount
    self.missed_quote_amount_was_set = true
  end

  private

  def validate_missed_quote_amount_was_set
    # FIXME: Required because we are using store_accessor and will_save_change_to_settings?
    # always returns true, at least in Rails 6.0
    return if settings_was == settings

    # Validating it this way forces us to manually call set_missed_quote_amount before saving into settings.
    # This involves less mental overhead than calling set_missed_quote_amount in the before_save callback as
    # we don't need to call internally all _was methods in all sub methods called within pending_quote_amount.
    unless missed_quote_amount_was_set
      errors.add(:missed_quote_amount,
                 'missed_quote_amount was not set, call set_missed_quote_amount before saving into settings')
    end

    self.missed_quote_amount_was_set = nil
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

    case exchange.minimum_amount_logic
    when :base_or_quote
      minimum_quote_size_in_base = ticker.minimum_quote_size / order_data[:rate]
      amount_type = minimum_quote_size_in_base < ticker.minimum_base_size ? :quote : :base
      amount = amount_type == :base ? order_data[:amount] : order_data[:quote_amount]
      minimum_amount = amount_type == :base ? ticker.minimum_base_size : ticker.minimum_quote_size
    when :base_and_quote
      minimum_amount = [ticker.minimum_quote_size / order_data[:rate], ticker.minimum_base_size].max
      amount_type = :base
      amount = order_data[:amount]
    end

    {
      amount_type: amount_type,
      amount: amount,
      below_minimum_amount: amount < minimum_amount
    }
  end
end
