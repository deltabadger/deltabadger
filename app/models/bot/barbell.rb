module Bot::Barbell
  extend ActiveSupport::Concern

  included do
    validate :validate_barbell_bot_settings, if: :barbell?

    def set_barbell_orders
      #  check exchange balances for base0 and base1
      #  convert balances to quote amount
      #  define order(s) amounts
      #  create order(s)
    end

    private

    def validate_barbell_bot_settings # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
      return if settings['quote_amount'].present? && settings['quote_amount'].to_f.positive? &&
                settings['quote'].present? &&
                settings['interval'].present? &&
                settings['base0'].present? &&
                settings['base1'].present? &&
                settings['allocation0'].present? && settings['allocation0'].to_f.between?(0, 1)

      errors.add(:settings, :invalid_settings, message: 'Invalid settings')
    end
  end
end
