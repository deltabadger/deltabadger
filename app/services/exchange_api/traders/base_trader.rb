module ExchangeApi
  module Traders
    class BaseTrader
      RECOVERABLE = { data: { recoverable: true }.freeze }.freeze
      NOT_FETCHED = { data: { fetched: false }.freeze }.freeze

      def buy
        raise NotImplementedError
      end

      def sell
        raise NotImplementedError
      end

      def currency_balance(currency, bot_id = nil)
        Result::Failure.new('Could not fetch account info')
      end

      def send_user_to_sendgrid(exchange_name, user)
        Result::Failure.new('Failed to save user to Sendgrid')
      end

      protected

      def error_to_failure(error)
        mapped_error = @map_errors.call(error)
        Result::Failure.new(
          *mapped_error.message, data: { recoverable: mapped_error.recoverable }
        )
      end
    end
  end
end
