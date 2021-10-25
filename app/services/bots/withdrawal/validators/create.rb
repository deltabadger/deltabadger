module Bots::Withdrawal::Validators
  class Create < BaseService
    def call(bot, user)
      bot_settings = BotSettings.new(bot.settings, user)

      if bot.valid? && bot_settings.valid?
        Result::Success.new
      else
        Result::Failure.new(
          *(bot.errors.full_messages + bot_settings.errors.full_messages)
        )
      end
    end

    class BotSettings
      include ActiveModel::Validations

      attr_reader :interval, :currency, :threshold, :threshold_enabled, :address, :interval_enabled

      validates :interval, :currency, :threshold, :address, presence: true
      validates :threshold_enabled, inclusion: { in: [true, false] }
      validates :interval_enabled, inclusion: { in: [true, false] }
      validate :validate_threshold
      validate :validate_interval

      def initialize(params, user)
        @interval = params['interval']
        @currency = params['currency']
        @threshold = params['threshold']
        @address = params['address']
        @threshold_enabled = params['threshold_enabled']
        @interval_enabled = params['interval_enabled']
        @hodler = user.subscription_name == 'hodler'
        @paid_plan = user.subscription_name == 'hodler' || user.subscription_name == 'investor'
      end

      private

      def validate_threshold
        return if !@threshold_enabled || @threshold.to_f.positive?

        errors.add(:threshold, ' cannot be negative')
      end

      def validate_interval
        return if !@interval_enabled || @interval.to_f.positive?

        errors.add(:interval, ' cannot be negative')
      end
    end
  end
end
