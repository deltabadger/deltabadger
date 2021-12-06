module Bots::Withdrawal::Validators
  class Create < BaseService
    def call(bot, user)
      bot_settings = BotSettings.new(bot.settings, user, bot)

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
      validate :threshold_above_minimum

      def initialize(params, user, bot)
        @interval = params['interval']
        @currency = params['currency']
        @threshold = params['threshold']
        @address = params['address']
        @threshold_enabled = params['threshold_enabled']
        @interval_enabled = params['interval_enabled']
        @hodler = user.subscription_name == 'hodler'
        @paid_plan = user.subscription_name == 'hodler' || user.subscription_name == 'investor'
        @minimums = GetWithdrawalMinimums.call({ exchange_id: bot.exchange_id }, user)
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

      def threshold_above_minimum
        return if !@threshold_enabled || !@minimums.success?

        return if @threshold.to_f >= @minimums.data[:minimum].to_f

        errors.add(:threshold, " should be greater than #{@minimums.data[:minimum]}")
      end
    end
  end
end
