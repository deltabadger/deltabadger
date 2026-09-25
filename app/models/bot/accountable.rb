module Bot::Accountable
  extend ActiveSupport::Concern

  included do
    store_accessor :transient_data,
                   :missed_quote_amount,
                   :missed_quote_amount_was_set

    validates :missed_quote_amount, numericality: { greater_than_or_equal_to: 0 }
    before_save :check_missed_quote_amount_was_set
  end

  def missed_quote_amount
    value = super
    value.present? ? value.to_d : 0
  end

  def pending_quote_amount
    return 0 if deleted?

    # The carry is a buy-side notion. While selling, freeze it: return the stored
    # missed_quote_amount unchanged so the sell cadence never corrupts the buy carry, and the
    # buy carry resumes intact on flip-back (invariant 3). try — Accountable is shared with
    # non-reversible bot types that have no selling? predicate. This MUST precede the started_at
    # guard below: a sell-side limit pause makes the decorated started_at nil, which would otherwise
    # return 0 here and wipe the frozen buy carry on the next set_missed_quote_amount.
    return missed_quote_amount if try(:selling?)

    return 0 if started_at.nil?

    calc_since = carry_window_marks.compact.max
    # The carry counts BUY investment only — a sell is divestment, not invested quote. Scope to buys
    # so a sell inside the window (e.g. just after a flip back to buying) can't be mistaken for quote
    # already invested and shrink the next buy. No-op for buy-only bot types (they never sell).
    closed_quote_amount = transactions.submitted.buy.regular
                                      .where('created_at >= ?', calc_since)
                                      .closed
                                      .pluck(:quote_amount_exec)
                                      .sum

    open_quote_amount = transactions.buy.regular
                                    .where('created_at >= ?', calc_since)
                                    .waiting
                                    .pluck(:quote_amount, :amount, :price)
                                    .map { |quote_amount, amount, price| quote_amount || (amount * price) }
                                    .sum

    total_quote_amount_invested = closed_quote_amount + open_quote_amount

    # Round to 6 decimal places to avoid floating point precision issues!
    intervals = ((last_interval_checkpoint_at.round(6) - calc_since.round(6)) / effective_interval_duration).floor + 1

    # puts "intervals: #{intervals}"
    # puts "last_interval_checkpoint_at: #{last_interval_checkpoint_at} (#{last_interval_checkpoint_at.to_f})"
    # puts "started_at:                  #{started_at} (#{started_at.to_f})"
    # puts "settings_changed_at:         #{settings_changed_at} (#{settings_changed_at.to_f})"
    # puts "calc_since:                  #{calc_since} (#{calc_since.to_f})"
    # puts "current_time:                #{Time.current}"
    # puts "real intervals since started_at: #{((last_interval_checkpoint_at - started_at) / effective_interval_duration).floor}"
    # puts "real intervals since settings_changed_at: #{((last_interval_checkpoint_at - settings_changed_at) / effective_interval_duration).floor}"
    # puts "intervals since started_at: #{((last_interval_checkpoint_at - started_at) / effective_interval_duration).floor + 1}"
    # puts "intervals since settings_changed_at: #{((last_interval_checkpoint_at - settings_changed_at) / effective_interval_duration).floor + 1}"
    # puts "missed_quote_amount: #{missed_quote_amount}"
    # puts "total_quote_amount_invested: #{total_quote_amount_invested}"
    # puts "quote_amount: #{quote_amount}"
    # puts "effective_quote_amount: #{effective_quote_amount}"
    # puts "interval: #{interval}"
    # puts "interval_duration: #{interval_duration}"
    # puts "effective_interval_duration: #{effective_interval_duration}"
    # puts "result: #{effective_quote_amount * intervals + missed_quote_amount - total_quote_amount_invested}"

    [(effective_quote_amount * intervals) + missed_quote_amount - total_quote_amount_invested, 0].max
  end

  # Any other assignment is the caller's own value for the carry (a fresh start's nil): it ends the
  # claim of a pending capture, which the guard would otherwise take back.
  def missed_quote_amount=(value)
    @carry_capture_pending = false
    super
  end

  # What the carry window is drawn from: pending_quote_amount counts intervals and investment from
  # the later of the two. A limit pause or resume moves the decorated started_at (to nil while
  # paused), a settings change moves settings_changed_at.
  def carry_window_marks
    [started_at, settings_changed_at]
  end

  # Captures what the bot owes up to now, measured under the settings as they are. The capture only
  # lands when the carry window moves with it — a settings change, a fresh start, a limit pause or
  # resume: saved with the window where it was, the owed intervals are counted twice, in the window
  # and in the carry. The guard judges that after the callbacks that rewrite settings on save (an
  # exchange switch repoints the ticker ids), so a caller that cannot know yet may just capture.
  def set_missed_quote_amount
    # Capturing again starts from the carry as it was before the first capture; measured on top of
    # the first, the owed intervals would be counted twice.
    if @carry_capture_pending
      self.missed_quote_amount = @carry_before_capture
    else
      @carry_before_capture = transient_data.to_h['missed_quote_amount']
      @carry_window_marks_at_capture = carry_window_marks
    end
    self.missed_quote_amount = pending_quote_amount
    self.missed_quote_amount_was_set = true
    @carry_capture_pending = true
  end

  private

  def check_missed_quote_amount_was_set
    captured = @carry_capture_pending
    @carry_capture_pending = false
    # Not will_save_change_to_settings? or settings_was: with store_accessor the first is true on
    # every save, and a load fills defaults a stored row lacks (Automation::Configurable).
    unless settings_changed_since_load?
      # No settings change. Unless the window moved another way (a limit pause or resume), a capture
      # made for this save must not land.
      if captured && carry_window_marks == @carry_window_marks_at_capture
        self.missed_quote_amount = @carry_before_capture
        self.missed_quote_amount_was_set = nil
      end
      return
    end

    # Validating it this way forces us to manually call set_missed_quote_amount before saving into settings.
    # This involves less mental overhead than calling set_missed_quote_amount directly in the before_save
    # callback as we don't need to call internally all _was methods in all sub methods called within
    # pending_quote_amount.
    # This is also a safety measure to raise an error if we attempt to save unwanted changes to settings.
    # Raise an error in the before_save instead of validate to avoid having to set_missed_quote_amount before
    # any .valid? call.
    unless missed_quote_amount_was_set
      Rails.logger.error(
        'Attempting to save settings with missed_quote_amount not set, call ' \
        "set_missed_quote_amount before saving: #{settings_was.inspect} != #{settings.inspect}"
      )
      raise 'Attempting to save settings with missed_quote_amount not set, call set_missed_quote_amount before saving'
    end

    # The capture lands, so the carry window restarts with it. Automation::Configurable restarts it
    # too, but runs before the callbacks that rewrite settings on save and misses those changes (an
    # exchange switch repointing the ticker ids).
    self.settings_changed_at = Time.current
    # Cap at the new effective_quote_amount to prevent accumulated debt from
    # carrying through schedule changes (e.g. switching to smart intervals).
    self.missed_quote_amount = [missed_quote_amount, effective_quote_amount].min
    self.missed_quote_amount_was_set = nil
  end
end
