module ExchangeApi
  module Clients
    class GetValidator < BaseService
      DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

      def initialize(exchanges_repository: ExchangesRepository.new)
        @exchanges_repository = exchanges_repository
      end

      def call(api_key)
        exchange = @exchanges_repository.find(api_key.exchange_id)

        return Fake.Validator.new if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'binance'
          Binance::Validator.new
        when 'bitbay'
          Bitbay::Validator.new
        when 'kraken'
          Kraken::Validator.new
        end
      end
    end
  end
end
