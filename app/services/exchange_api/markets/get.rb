module ExchangeApi
  module Markets
    class Get < BaseService
      include ExchangeApi::BinanceEnum

      # rubocop:disable Metrics/CyclomaticComplexity
      def call(exchange_id)
        exchange = Exchange.find(exchange_id)
        return ExchangeApi::Markets::Fake::Market.new if Rails.configuration.dry_run

        case exchange.name.downcase
        when 'binance' then ExchangeApi::Markets::Binance::Market.new(url_base: EU_URL_BASE)
        when 'binance.us' then ExchangeApi::Markets::Binance::Market.new(url_base: US_URL_BASE)
        when 'zonda' then ExchangeApi::Markets::Zonda::Market.new
        when 'kraken' then ExchangeApi::Markets::Kraken::Market.new
        when 'coinbase' then ExchangeApi::Markets::Coinbase::Market.new
        when 'gemini' then ExchangeApi::Markets::Gemini::Market.new
        when 'bitso' then ExchangeApi::Markets::Bitso::Market.new
        when 'kucoin' then ExchangeApi::Markets::Kucoin::Market.new
        when 'bitfinex' then ExchangeApi::Markets::Bitfinex::Market.new
        when 'bitstamp' then ExchangeApi::Markets::Bitstamp::Market.new
        end
      end
      # rubocop:enable Metrics/CyclomaticComplexity
    end
  end
end
