module BotsManager::Withdrawal::Validators
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
      validate :validate_whitelist

      def initialize(params, user, bot)
        @interval = params['interval']
        @currency = params['currency']
        @threshold = params['threshold']
        @address = params['address']
        @threshold_enabled = params['threshold_enabled']
        @interval_enabled = params['interval_enabled']
        @pro = user.subscription.pro?
        @legendary = user.subscription.legendary?
        @paid_plan = user.subscription.paid?
        @minimums = GetWithdrawalMinimums.call({ exchange_id: bot.exchange_id }, user)
        @withdrawal_info_processor = get_withdrawal_info_processor(user.api_keys, bot.exchange_id)
        @exchange_id = bot.exchange_id
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

        minimum_check_params = { minimum: @minimums.data[:minimum].to_f, currency: @currency, threshold: @threshold.to_f }
        return if CheckWithdrawalMinimums.call(@exchange_id, minimum_check_params)

        errors.add(:threshold, ' is to small.')
      end

      def validate_whitelist
        available_wallets = @withdrawal_info_processor.available_wallets
        return if !available_wallets.success? || available_wallets.data.nil?

        return if available_wallets.data.map { |w| w[:address] }.include?(@address)

        errors.add(:address, ' is not whitelisted.')
      end

      def get_withdrawal_info_processor(api_keys, exchange_id)
        api_key = api_keys.find_by(exchange_id: exchange_id, key_type: 'withdrawal')
        return nil unless api_key.present?

        ExchangeApi::WithdrawalInfo::Get.call(api_key)
      end
    end
  end
end
