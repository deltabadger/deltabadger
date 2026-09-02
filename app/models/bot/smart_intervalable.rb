module Bot::SmartIntervalable
  extend ActiveSupport::Concern

  included do
    store_accessor :settings,
                   :smart_intervaled,
                   :smart_interval_quote_amount,
                   :smart_interval_base_amount

    after_initialize :initialize_smart_intervalable_settings
    # The sell base split can't be seeded at load when sell_amount is still blank (e.g. just after a
    # flip). Seed it the moment a sell amount exists — before validation — so entering the sell amount
    # in the main sentence never trips the base-split presence check.
    before_validation :seed_smart_interval_base_amount, if: -> { smart_intervaled? && sells_base_amount? }

    validates :smart_intervaled, inclusion: { in: [true, false] }
    # Quote split governs buying; base split governs BASE-denominated selling. Direction-gate so a
    # selling bot is never blocked by a missing quote amount (and vice versa). The base check
    # additionally waits for a sell amount, so flipping a smart-on bot with no sell sentence yet
    # can't fail validation. A quote-denominated sell validates NEITHER split — the rule is not
    # offered in that mode (see Bot::Reversible#effective_interval_duration).
    #
    # The floor is a separate validation rather than numericality's greater_than_or_equal_to: that
    # option's `message` is validator-wide, so wiring the explanation into it would answer "abc"
    # with a minimum-order-size sentence instead of "is not a number".
    validates :smart_interval_quote_amount, numericality: true,
                                            if: -> { smart_intervaled? && !selling? }
    validate :validate_smart_interval_quote_minimum,
             if: -> { smart_intervaled? && !selling? }
    validates :smart_interval_base_amount, numericality: true,
                                           if: -> { smart_intervaled? && sells_base_amount? && sell_amount.present? }
    validate :validate_smart_interval_base_minimum,
             if: -> { smart_intervaled? && sells_base_amount? && sell_amount.present? }

    decorators = Module.new do
      def parse_params(params)
        super(params).merge(
          smart_intervaled: params[:smart_intervaled].presence&.in?(%w[1 true]),
          smart_interval_quote_amount: params[:smart_interval_quote_amount].presence&.to_f,
          smart_interval_base_amount: params[:smart_interval_base_amount].presence&.to_f
        ).compact
      end

      def effective_quote_amount
        return super unless smart_intervaled? &&
                            smart_interval_quote_amount.present?

        smart_interval_quote_amount
      end

      def effective_interval_duration
        return super unless smart_intervaled? &&
                            smart_interval_quote_amount.present? &&
                            quote_amount.present?

        # effective_interval_duration is an ActiveSupport::Duration. However, for some durations, after this
        # division, addition in other methods (e.g. Time.current + effective_interval_duration) stops working.
        # Re-converting it to seconds makes the addition work. Do NOT remove the .seconds !
        (super / (quote_amount / smart_interval_quote_amount.to_f)).seconds
      end
    end

    prepend decorators
  end

  def smart_intervaled?
    smart_intervaled == true
  end

  # The floor a split may not go under, and WHY. The form's `min`, the form's message and the
  # validation all read this one method, so the number they enforce and the sentence they show
  # cannot disagree. `kind` is :quote (buying) or :base (selling a base amount).
  #
  # Three floors compete and the binding one names the reason, resolved :exchange, :frequency,
  # :precision so the most concrete explanation wins a tie. The frequency term is compared
  # UNROUNDED: round_up of any positive frequency already lands on at least 10**-decimals, so
  # comparing the rounded value would make :precision unreachable even where precision is what
  # actually set the floor.
  def smart_interval_minimum(kind)
    return { value: 0, reason: :none } if tickers.empty?

    decimals = kind == :base ? least_precise_base_decimals : least_precise_quote_decimals
    frequency = smart_interval_frequency_minimum(kind)
    precision = 1.0 / (10**decimals)
    exchange = kind == :base ? minimum_base_for_exchange : minimum_for_exchange

    reason = if exchange >= frequency && exchange >= precision
               :exchange
             elsif frequency >= precision
               :frequency
             else
               :precision
             end

    {
      value: [Utilities::Number.round_up(frequency, precision: decimals), precision, exchange].max,
      reason: reason
    }
  end

  # The sentence that explains the floor, shown inline by the form and repeated in the flash when a
  # request gets past the browser. nil when there is nothing to explain — the browser's own wording
  # then stands.
  def smart_interval_minimum_message(kind)
    minimum = smart_interval_minimum(kind)
    return if minimum[:reason] == :none

    currency = (kind == :base ? base_asset : quote_asset)&.symbol
    amount = BigDecimal(minimum[:value].to_s).to_s('F').sub(/([0-9]\d*)\.0$/, '\\1')

    case minimum[:reason]
    when :exchange
      I18n.t('bot.smart_intervals_disclaimer', exchange: exchange&.name, currency: currency, minimum: amount)
    when :frequency
      I18n.t('bot.smart_intervals_minimum_frequency', currency: currency, minimum: amount)
    when :precision
      I18n.t('bot.smart_intervals_minimum_precision', currency: currency, minimum: amount,
                                                      decimals: kind == :base ? least_precise_base_decimals : least_precise_quote_decimals)
    end
  end

  private

  def validate_smart_interval_quote_minimum
    validate_smart_interval_minimum(:quote, :smart_interval_quote_amount)
  end

  def validate_smart_interval_base_minimum
    validate_smart_interval_minimum(:base, :smart_interval_base_amount)
  end

  # Parse rather than assume a Numeric: a split stored through the JSON settings column comes back
  # as the STRING "0.2", which numericality accepts and which raises on a comparison with a
  # BigDecimal. Skip only what will not parse at all — numericality has already spoken for those.
  # An is_a?(Numeric) guard here would instead quietly stop enforcing the floor on every such row.
  def validate_smart_interval_minimum(kind, attribute)
    value = BigDecimal(public_send(attribute).to_s, exception: false)
    return if value.nil?

    minimum = smart_interval_minimum(kind)
    return if value >= BigDecimal(minimum[:value].to_s)

    message = smart_interval_minimum_message(kind)
    if message
      errors.add(attribute, message)
    else
      errors.add(attribute, :greater_than_or_equal_to, count: minimum[:value])
    end
  end

  # Smallest split that still leaves at least `maximum_frequency` between two orders.
  def smart_interval_frequency_minimum(kind)
    maximum_frequency = 300 # seconds — at most one order every 5 minutes

    if kind == :base
      amount = try(:sell_amount).to_f
      interval_secs = (Automation::Schedulable::INTERVALS[try(:sell_interval)] || interval_duration).to_f
      return 0 unless amount.positive? && interval_secs.positive?

      amount / interval_secs * maximum_frequency
    else
      return 0 if quote_amount.blank?

      quote_amount / interval_duration.to_f * maximum_frequency
    end
  end

  def initialize_smart_intervalable_settings
    self.smart_intervaled ||= false
    self.smart_interval_quote_amount ||= if quote_amount.present? && tickers.present?
                                           [
                                             quote_amount / 10,
                                             minimum_smart_interval_quote_amount * 10
                                           ].max.round(least_precise_quote_decimals).to_f
                                         end
    # The sell base split is NOT seeded here (that would write settings on load and dirty an existing
    # bot). It is seeded by before_validation :seed_smart_interval_base_amount, which only runs while
    # selling+smart with a sell amount — exactly when it's needed and safe to persist.
  end

  # Seed the sell base split from the sell amount (mirror of the buy seed) when it's needed but still
  # blank, so the user never has to open the Smart Intervals rule before setting a sell amount.
  def seed_smart_interval_base_amount
    return if smart_interval_base_amount.present?
    return if try(:sell_amount).blank?

    # .to_f so the split persists as a plain number: sell_amount is a BigDecimal, and a BigDecimal
    # serializes into the JSON settings column as a String (precision-preserving), which then breaks
    # Float math in effective_interval_duration on the next load.
    self.smart_interval_base_amount = [
      sell_amount / 10,
      minimum_smart_interval_base_amount * 10
    ].max.round(least_precise_base_decimals).to_f
  end

  def minimum_smart_interval_base_amount
    smart_interval_minimum(:base)[:value]
  end

  def least_precise_base_decimals
    @least_precise_base_decimals ||= tickers.pluck(:base_decimals).compact.min
  end

  def minimum_smart_interval_quote_amount
    smart_interval_minimum(:quote)[:value]
  end

  # Override in subclasses to set exchange-specific minimums
  # For Index bots, this returns the highest minimum_quote_size among tickers
  def minimum_for_exchange
    0
  end

  # The venue's own floor on the SELL side. Still 0 everywhere: DcaSingleAsset is the only type that
  # sells, and giving it a ticker-derived floor here would make the numericality check reject splits
  # already stored on live bots — including during Bot::Lifecycle#stop, which goes through `update`.
  # That change needs its own backfill; see docs/superpowers/plans/2026-09-02-conversational-validation-feedback.md.
  def minimum_base_for_exchange
    0
  end

  def least_precise_quote_decimals
    @least_precise_quote_decimals ||= tickers.pluck(:quote_decimals).compact.min
  end
end
