module ExchangeApi
  module Markets
    class Get < BaseService
      DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

      def initialize(exchanges_repository: ExchangesRepository.new)
        @exchanges_repository = exchanges_repository
      end

      def call(exchange_id)
        exchange = @exchanges_repository.find(exchange_id)
        return ExchangeApi::Markets::Fake::Market.new if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'binance'
          ExchangeApi::Markets::Binance::Market.new
        when 'binanceus'
          ExchangeApi::Markets::BinanceUS::Market.new
        when 'bitbay'
          ExchangeApi::Markets::Bitbay::Market.new
        when 'kraken'
          ExchangeApi::Markets::Kraken::Market.new
        end
      end
    end
  end
end
