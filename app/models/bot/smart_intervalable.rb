module Bot::SmartIntervalable
  extend ActiveSupport::Concern

  included do # rubocop:disable Metrics/BlockLength
    store_accessor :settings,
                   :smart_intervaled,
                   :smart_interval_quote_amount

    after_initialize :initialize_smart_intervalable_settings

    validates :smart_intervaled, inclusion: { in: [true, false] }
    validates :smart_interval_quote_amount,
              numericality: { greater_than_or_equal_to: ->(b) { b.minimum_quote_amount_limit } },
              if: :smart_intervaled

    decorators = Module.new do
      def interval_duration
        return super unless smart_intervaled? &&
                            smart_interval_quote_amount.present? &&
                            quote_amount.present?

        super / (quote_amount.to_f / smart_interval_quote_amount)
      end

      def pending_quote_amount
        return super unless smart_intervaled? &&
                            smart_interval_quote_amount.present?

        quote_amount_bak = quote_amount
        self.quote_amount = smart_interval_quote_amount
        value = super
        self.quote_amount = quote_amount_bak
        value
      end

      def set_missed_quote_amount
        return super unless settings_was['smart_intervaled'] &&
                            settings_was['smart_interval_quote_amount'].present?

        quote_amount_was_bak = settings_was['quote_amount']
        settings_was['quote_amount'] = settings_was['smart_interval_quote_amount']
        super
        settings_was['quote_amount'] = quote_amount_was_bak
      end
    end

    prepend decorators
  end

  def smart_intervaled?
    smart_intervaled == true
  end

  def smart_interval
    return nil unless smart_intervaled?
    return nil if smart_interval_quote_amount.blank? || quote_amount.blank? || interval.blank?

    interval_duration / (quote_amount.to_f / smart_interval_quote_amount.to_f)
  end

  private

  def initialize_smart_intervalable_settings
    self.smart_intervaled ||= false
    self.smart_interval_quote_amount ||= nil
  end

  def minimum_smart_interval_quote_amount
    least_precise_quote_decimals = tickers.pluck(:quote_decimals).compact.min
    @minimum_smart_interval_quote_amount ||= 1.0 / (10**least_precise_quote_decimals)
  end
end
