module Bot::Accountable
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
end
