module ExchangeApi
  module MapErrors
    class Binance < ExchangeApi::MapErrors::Base
      # rubocop:disable Metrics/LineLength
      def errors_mapping
        {
          'An unknown error occured while processing the request.' => Error.new('Unknown error', true),
          'Internal error; unable to process your request. Please try again.' => Error.new('Unknown error', true),
          'Timeout waiting for response from backend server. Send status unknown; execution status unknown.' => Error.new('Request timed out', true),
          'Invalid API-key, IP, or permissions for action.' => Error.new('Insufficient permissions or unverified account.', false),
          'Account has insufficient balance for requested action.' => Error.new('Insufficient funds', false),
          'Filter failure: LOT_SIZE' => Error.new('Offer funds are not exceeding minimums', false),
          'Filter failure: MIN_NOTIONAL' => Error.new('Offer funds are not exceeding minimums', false),
          'Filter failure: PRICE_FILTER' => Error.new('Offer funds are not exceeding minimums', false)
        }
      end
      # rubocop:enable Metrics/LineLength
    end
  end
end
