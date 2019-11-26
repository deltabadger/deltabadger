module Bots::Free::Validators
  class Create < BaseService
    def call(bot)
      bot_settings = BotSettings.new(bot.settings)
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

      attr_reader :interval, :currency, :type, :price

      CURRENCIES = %w[USD EUR PLN].freeze
      INTERVALS = %w[month week day hour minute].freeze
      TYPES = %w[buy sell].freeze

      validates :interval, :currency, :type, :price, presence: true
      validates :currency, inclusion: { in: CURRENCIES }
      validates :interval, inclusion: { in: INTERVALS }
      validates :price, numericality: { only_float: true, greater_than: 0 }

      def initialize(params)
        @interval = params.fetch('interval')
        @currency = params.fetch('currency')
        @type = params.fetch('type')
        @price = params.fetch('price').to_f
      end
    end
  end
end
