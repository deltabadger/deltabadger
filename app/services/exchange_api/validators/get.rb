module ExchangeApi
  module Validators
    class Get < BaseService
      include ExchangeApi::BinanceEnum
      DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

      def initialize(exchanges_repository: ExchangesRepository.new)
        @exchanges_repository = exchanges_repository
      end

      def call(exchange_id)
        exchange = @exchanges_repository.find(exchange_id)

        return Fake::Validator.new if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'binance'
          Binance::Validator.new(url_base: EU_URL_BASE)
        when 'binance.us'
          Binance::Validator.new(url_base: US_URL_BASE)
        when 'bitbay'
          Bitbay::Validator.new
        when 'kraken'
          Kraken::Validator.new
        when 'coinbase pro'
          CoinbasePro::Validator.new
        when 'gemini'
          Gemini::Validator.new
        when 'ftx'
          Ftx::Validator.new
        end
      end
    end
  end
end
