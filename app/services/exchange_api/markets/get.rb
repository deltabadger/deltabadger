module ExchangeApi
  module Markets
    class Get < BaseService
      include ExchangeApi::BinanceEnum
      include ExchangeApi::FtxEnum
      DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

      def initialize(exchanges_repository: ExchangesRepository.new)
        @exchanges_repository = exchanges_repository
      end

      def call(exchange_id)
        exchange = @exchanges_repository.find(exchange_id)
        return ExchangeApi::Markets::Fake::Market.new if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'binance'
          ExchangeApi::Markets::Binance::Market.new(url_base: EU_URL_BASE)
        when 'binance.us'
          ExchangeApi::Markets::Binance::Market.new(url_base: US_URL_BASE)
        when 'bitbay'
          ExchangeApi::Markets::Bitbay::Market.new
        when 'kraken'
          ExchangeApi::Markets::Kraken::Market.new
        when 'coinbase pro'
          ExchangeApi::Markets::CoinbasePro::Market.new
        when 'gemini'
          ExchangeApi::Markets::Gemini::Market.new
        when 'ftx'
          ExchangeApi::Markets::Ftx::Market.new(url_base: FTX_EU_URL_BASE)
        when 'ftx.us'
          ExchangeApi::Markets::Ftx::Market.new(url_base: FTX_US_URL_BASE)
        when 'bitso'
          ExchangeApi::Markets::Bitso::Market.new
        when 'kucoin'
          ExchangeApi::Markets::Kucoin::Market.new
        when 'bitfinex'
          ExchangeApi::Markets::Bitfinex::Market.new
        end
      end
    end
  end
end
