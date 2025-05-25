module Bot::SmartIntervalable
  extend ActiveSupport::Concern

  included do # rubocop:disable Metrics/BlockLength
    store_accessor :settings,
                   :smart_intervaled,
                   :smart_interval_quote_amount

    store_accessor :transient_data,
                   :normal_interval_quote_amount,
                   :normal_interval_duration

    after_initialize :initialize_smart_intervalable_settings
    after_initialize :set_normal_interval_backup_values
    before_save :set_normal_interval_backup_values, if: :will_save_change_to_settings?

    validates :smart_intervaled, inclusion: { in: [true, false] }
    validates :smart_interval_quote_amount,
              numericality: { greater_than_or_equal_to: :minimum_smart_interval_quote_amount },
              if: :smart_intervaled

    decorators = Module.new do
      def interval_duration
        return super unless smart_intervaled? &&
                            smart_interval_quote_amount.present? &&
                            normal_interval_quote_amount.present?

        super / (normal_interval_quote_amount.to_f / smart_interval_quote_amount)
      end

      def pending_quote_amount
        return super unless smart_intervaled? &&
                            smart_interval_quote_amount.present? &&
                            normal_interval_quote_amount.present?

        quote_amount_bak = quote_amount
        self.quote_amount = smart_interval_quote_amount
        value = super
        self.quote_amount = quote_amount_bak
        value
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

    interval_duration / (quote_amount.to_f / smart_interval_quote_amount)
  end

  private

  def initialize_smart_intervalable_settings
    self.smart_intervaled ||= false
    self.smart_interval_quote_amount ||= nil
  end

  def minimum_smart_interval_quote_amount
    # the minimum amount would set one order every 1 minute
    minimum_for_frequency = if quote_amount.present?
                              quote_amount / 1.public_send(interval) * 60
                            else
                              0
                            end

    least_precise_quote_decimals = tickers.pluck(:quote_decimals).compact.min
    minimum_for_precision = 1.0 / (10**least_precise_quote_decimals)

    [minimum_for_frequency, minimum_for_precision].max
  end

  def set_normal_interval_backup_values
    self.normal_interval_quote_amount = quote_amount
    self.normal_interval_duration = interval.present? ? 1.public_send(interval) : nil
  end
end
