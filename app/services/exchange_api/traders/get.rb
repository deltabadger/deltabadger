module ExchangeApi
  module Traders
    class Get < BaseService
      DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

      def initialize(exchanges_repository: ExchangesRepository.new)
        @exchanges_repository = exchanges_repository
      end

      def call(api_key, order_type)
        exchange = @exchanges_repository.find(api_key.exchange_id)
        return fake_client(order_type, exchange.name) if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'binance'
          binance_client(order_type)
        when 'bitbay'
          bitbay_client(order_type)
        when 'kraken'
          kraken_client(order_type)
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

      def binance_client(order_type)
        client = if limit_trader?(order_type)
                   ExchangeApi::Traders::Binance::LimitTrader
                 else
                   ExchangeApi::Traders::Binance::MarketTrader
                 end
        client.new(
          api_key: api_key.key,
          api_secret: api_key.secret
        )
      end

      def bitbay_client(order_type)
        client = if limit_trader?(order_type)
                   ExchangeApi::Traders::Bitbay::LimitTrader
                 else
                   ExchangeApi::Traders::Bitbay::MarketTrader
                 end
        client.new(
          api_key: api_key.key,
          api_secret: api_key.secret
        )
      end

      def kraken_client(order_type)
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

      def market_trader?(order_type)
        order_type == 'market'
      end

      def limit_trader?(order_type)
        !market_trader?(order_type)
      end
    end
  end
end
