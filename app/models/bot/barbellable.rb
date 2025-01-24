module Bot::Barbellable
  extend ActiveSupport::Concern

  included do
    validate :validate_barbell_bot_settings, if: :barbell?

    private

    def validate_barbell_bot_settings
      return if settings['quote_amount'].present? && settings['quote_amount'].to_f > 0 &&
                settings['quote'].present? &&
                settings['interval'].present? &&
                settings['base0'].present? &&
                settings['base1'].present? &&
                settings['allocation0'].present? && settings['allocation0'].to_f >= 0 && settings['allocation0'].to_f <= 1

      puts 'barbellable errors!!'

      errors.add(:settings, :invalid_settings, message: 'Invalid settings')
    end
  end
end
