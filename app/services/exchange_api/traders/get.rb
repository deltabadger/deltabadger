module ExchangeApi
  module Traders
    class Get < BaseService
      include ExchangeApi::BinanceEnum

      def call(api_key, order_type)
        exchange = Exchange.find(api_key.exchange_id)
        return fake_client(order_type, exchange.name) if Rails.configuration.dry_run

        case exchange.name.downcase
        when 'binance' then binance_client(api_key, order_type, EU_URL_BASE)
        when 'binance.us' then binance_client(api_key, order_type, US_URL_BASE)
        when 'kraken' then kraken_client(api_key, order_type)
        when 'coinbase' then coinbase_client(api_key, order_type)
        end
      end

      private

      def fake_client(order_type, exchange_name)
        client = if limit_trader?(order_type)
                   ExchangeApi::Traders::Fake::LimitTrader
                 else
                   ExchangeApi::Traders::Fake::MarketTrader
                 end
        client.new(exchange_name)
      end

      def binance_client(api_key, order_type, url_base)
        client = if limit_trader?(order_type)
                   ExchangeApi::Traders::Binance::LimitTrader
                 else
                   ExchangeApi::Traders::Binance::MarketTrader
                 end
        client.new(
          api_key: api_key.key,
          api_secret: api_key.secret,
          url_base:
        )
      end

      def kraken_client(api_key, order_type)
        client = if limit_trader?(order_type)
                   ExchangeApi::Traders::Kraken::LimitTrader
                 else
                   ExchangeApi::Traders::Kraken::MarketTrader
                 end
        client.new(
          api_key: api_key.key,
          api_secret: api_key.secret,
          options: { german_trading_agreement: api_key.german_trading_agreement }
        )
      end

      def coinbase_client(api_key, order_type)
        client = if limit_trader?(order_type)
                   ExchangeApi::Traders::Coinbase::LimitTrader
                 else
                   ExchangeApi::Traders::Coinbase::MarketTrader
                 end
        client.new(
          api_key: api_key.key,
          api_secret: api_key.secret
        )
      end

      def market_trader?(order_type)
        order_type == 'market'
      end

      def limit_trader?(order_type)
        !market_trader?(order_type)
      end
    end
  end
end
