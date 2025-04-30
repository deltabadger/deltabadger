module ExchangeApi
  module Markets
    class Get < BaseService
      include ExchangeApi::BinanceEnum
      include ExchangeApi::FtxEnum
      DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

      # rubocop:disable Metrics/CyclomaticComplexity
      def call(exchange_id)
        exchange = Exchange.find(exchange_id)
        return ExchangeApi::Markets::Fake::Market.new if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'binance' then ExchangeApi::Markets::Binance::Market.new(url_base: EU_URL_BASE)
        when 'binance.us' then ExchangeApi::Markets::Binance::Market.new(url_base: US_URL_BASE)
        when 'zonda' then ExchangeApi::Markets::Zonda::Market.new
        when 'kraken' then ExchangeApi::Markets::Kraken::Market.new
        when 'coinbase pro' then ExchangeApi::Markets::CoinbasePro::Market.new
        when 'coinbase' then ExchangeApi::Markets::Coinbase::Market.new
        when 'gemini' then ExchangeApi::Markets::Gemini::Market.new
        when 'ftx' then ExchangeApi::Markets::Ftx::Market.new(url_base: FTX_EU_URL_BASE)
        when 'ftx.us' then ExchangeApi::Markets::Ftx::Market.new(url_base: FTX_US_URL_BASE)
        when 'bitso' then ExchangeApi::Markets::Bitso::Market.new
        when 'kucoin' then ExchangeApi::Markets::Kucoin::Market.new
        when 'bitfinex' then ExchangeApi::Markets::Bitfinex::Market.new
        when 'bitstamp' then ExchangeApi::Markets::Bitstamp::Market.new
        when 'probit global' then ExchangeApi::Markets::Probit::Market.new
        when 'probit' then ExchangeApi::Markets::Probit::Market.new
        end
      end
      # rubocop:enable Metrics/CyclomaticComplexity
    end
  end
end
