module Bots::DcaSingleAsset::OrderSetter # rubocop:disable Metrics/ModuleLength
  extend ActiveSupport::Concern

  included do
    store_accessor :transient_data,
                   :missed_quote_amount,
                   :missed_quote_amount_was_set

    validates :missed_quote_amount, numericality: { greater_than_or_equal_to: 0 }
    before_save :check_missed_quote_amount_was_set, if: :will_save_change_to_settings?
  end

  def missed_quote_amount
    value = super
    value.present? ? value.to_d : 0
  end

  def set_order(
    order_amount_in_quote:,
    update_missed_quote_amount: false
  )
    Rails.logger.info("set_order for bot #{id} with order_amount_in_quote: #{order_amount_in_quote}, update_missed_quote_amount: #{update_missed_quote_amount}")
    raise StandardError, 'quote_amount is required' if order_amount_in_quote.blank?
    raise StandardError, 'quote_amount must be positive' if order_amount_in_quote.negative?
    return Result::Success.new if order_amount_in_quote.zero? || order_amount_in_quote.negative?

    result = get_order(order_amount_in_quote)
    unless result.success?
      Rails.logger.error("set_order for bot #{id} failed to get order: #{result.errors.inspect}")
      create_failed_order!({
                             base_asset: base_asset,
                             quote_asset: quote_asset,
                             error_messages: result.errors
                           })
      return result
    end

    order_data = result.data
    Rails.logger.info("set_order for bot #{id} got order_data: #{order_data.inspect}")

    if order_data[:amount].zero?
      Rails.logger.info("set_order for bot #{id} ignoring order #{order_data.inspect}")
      return Result::Success.new
    end

    amount_info = calculate_best_amount_info(order_data)
    if amount_info[:below_minimum_amount]
      Rails.logger.info("set_order for bot #{id} creating skipped order #{order_data.inspect}")
      create_skipped_order!(order_data)
      return Result::Success.new
    end

    Rails.logger.info("set_order for bot #{id} creating order #{order_data.inspect} with amount info #{amount_info.inspect}")

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
      Rails.logger.info("set_order for bot #{id} created order #{order_id}")
      Bot::FetchAndCreateOrderJob.perform_later(self, order_id)
      update!(missed_quote_amount: [0, missed_quote_amount - order_data[:quote_amount]].max) if update_missed_quote_amount
    else
      Rails.logger.error("set_order for bot #{id} failed to create order #{order_data.inspect}: #{result.errors.inspect}")
      create_failed_order!(order_data.merge!(error_messages: result.errors))
      return result
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

    # Round to 6 decimal places to avoid floating point precision issues!
    intervals = ((last_interval_checkpoint_at.round(6) - calc_since.round(6)) / interval_duration).floor + 1

    # puts "intervals: #{intervals}"
    # puts "last_interval_checkpoint_at: #{last_interval_checkpoint_at} (#{last_interval_checkpoint_at.to_f})"
    # puts "started_at:                  #{started_at} (#{started_at.to_f})"
    # puts "settings_changed_at:         #{settings_changed_at} (#{settings_changed_at.to_f})"
    # puts "calc_since:                  #{calc_since} (#{calc_since.to_f})"
    # puts "current_time:                #{Time.current}"
    # puts "real intervals since started_at: #{((last_interval_checkpoint_at - started_at) / interval_duration).floor}"
    # puts "real intervals since settings_changed_at: #{((last_interval_checkpoint_at - settings_changed_at) / interval_duration).floor}"
    # puts "intervals since started_at: #{((last_interval_checkpoint_at - started_at) / interval_duration).floor + 1}"
    # puts "intervals since settings_changed_at: #{((last_interval_checkpoint_at - settings_changed_at) / interval_duration).floor + 1}"
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

  def check_missed_quote_amount_was_set
    # FIXME: Required because we are using store_accessor and will_save_change_to_settings?
    # always returns true, at least in Rails 6.0
    return if settings_was == settings

    # Validating it this way forces us to manually call set_missed_quote_amount before saving into settings.
    # This involves less mental overhead than calling set_missed_quote_amount directly in the before_save
    # callback as we don't need to call internally all _was methods in all sub methods called within
    # pending_quote_amount.
    # Raise an error in the before_save instead of validate to avoid having to set_missed_quote_amount before
    # any .valid? call.
    unless missed_quote_amount_was_set
      raise 'Attempting to save settings with missed_quote_amount not set, call set_missed_quote_amount before saving'
    end

    self.missed_quote_amount_was_set = nil
  end

  def get_order(order_amount_in_quote)
    result = exchange.get_ask_price(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
    return result unless result.success?

    Result::Success.new(calculate_order_data(
                          price: result.data,
                          order_amount_in_quote: order_amount_in_quote
                        ))
  end

  def calculate_order_data(price:, order_amount_in_quote:)
    order_size_in_base = order_amount_in_quote / price
    {
      base_asset: base_asset,
      quote_asset: quote_asset,
      rate: price,
      amount: order_size_in_base,
      quote_amount: order_amount_in_quote
    }
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
