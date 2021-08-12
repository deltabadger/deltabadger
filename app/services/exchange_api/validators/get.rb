module ExchangeApi
  module Validators
    class Get < BaseService
      include ExchangeApi::BinanceEnum
      include ExchangeApi::FtxEnum
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
          Ftx::Validator.new(url_base: FTX_EU_URL_BASE)
        when 'ftx.us'
          Ftx::Validator.new(url_base: FTX_US_URL_BASE)
        when 'bitso'
          Bitso::Validator.new
        when 'kucoin'
          Kucoin::Validator.new
        when 'bitfinex'
          Bitfinex::Validator.new
        when 'bitstamp'
          Bitstamp::Validator.new
        end
      end
    end
  end
end
