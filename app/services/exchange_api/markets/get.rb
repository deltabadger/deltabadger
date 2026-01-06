module ExchangeApi
  module Markets
    class Get < BaseService
      include ExchangeApi::BinanceEnum

      def call(exchange_id)
        exchange = Exchange.find(exchange_id)
        return ExchangeApi::Markets::Fake::Market.new if Rails.configuration.dry_run

        case exchange.name.downcase
        when 'binance' then ExchangeApi::Markets::Binance::Market.new(url_base: EU_URL_BASE)
        when 'binance.us' then ExchangeApi::Markets::Binance::Market.new(url_base: US_URL_BASE)
        when 'kraken' then ExchangeApi::Markets::Kraken::Market.new
        when 'coinbase' then ExchangeApi::Markets::Coinbase::Market.new
        end
      end
    end
  end
end
