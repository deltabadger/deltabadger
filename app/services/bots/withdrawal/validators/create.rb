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

      attr_reader :interval, :currency, :threshold, :threshold_enabled, :address

      validates :interval, :currency, :threshold, :threshold_enabled, :address, presence: true
      validates :interval, numericality: { only_float: true, greater_than: 0 }
      validates :threshold, numericality: { only_float: true, greater_than: 0 }
      validates :threshold_enabled, inclusion: { in: [true, false] }

      def initialize(params, user)
        @interval = params['interval']
        @currency = params['currency']
        @threshold = params['threshold']
        @address = params['address']
        @threshold_enabled = params['threshold_enabled']
        @hodler = user.subscription_name == 'hodler'
        @paid_plan = user.subscription_name == 'hodler' || user.subscription_name == 'investor'
      end
    end
  end
end
