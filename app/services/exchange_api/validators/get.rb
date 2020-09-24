module ExchangeApi
  module Validators
    class Get < BaseService
      DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

      def initialize(exchanges_repository: ExchangesRepository.new)
        @exchanges_repository = exchanges_repository
      end

      def call(api_key)
        exchange = @exchanges_repository.find(api_key.exchange_id)

        return ExchangeApi::Validators::Fake.new if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'binance'
          ExchangeApi::Validators::Binance.new
        when 'bitbay'
          ExchangeApi::Validators::Bitbay.new
        when 'kraken'
          ExchangeApi::Validators::Kraken.new
        end
      end
    end
  end
end
