module ExchangeApi
  module Clients
    class GetTrader < BaseService
      DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

      def initialize(exchanges_repository: ExchangesRepository.new)
        @exchanges_repository = exchanges_repository
      end

      def call(api_key, order_type)
        exchange = @exchanges_repository.find(api_key.exchange_id)
        return get_fake_client(order_type, exchange.name) if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'binance'
          get_binance_client(order_type)
        when 'bitbay'
          get_bitbay_client(order_type)
        when 'kraken'
          get_kraken_client(order_type)
        end
      end

      private

      def get_fake_client(order_type, exchange_name)
        client = if limit_trader?(order_type)
                   ExchangeApi::Clients::Fake::LimitTrader
                 else
                   ExchangeApi::Clients::Fake::MarketTrader
                 end
        client.new(exchange_name)
      end

      def get_binance_client(order_type)
        client = if limit_trader?(order_type)
                   ExchangeApi::Clients::Binance::LimitTrader
                 else
                   ExchangeApi::Clients::Binance::MarketTrader
                 end
        client.new(
          api_key: api_key.key,
          api_secret: api_key.secret
        )
      end

      def get_bitbay_client(order_type)
        client = if limit_trader?(order_type)
                   ExchangeApi::Clients::Bitbay::LimitTrader
                 else
                   ExchangeApi::Clients::Bitbay::MarketTrader
                 end
        client.new(
          api_key: api_key.key,
          api_secret: api_key.secret
        )
      end

      def get_kraken_client(order_type)
        client = if limit_trader?(order_type)
                   ExchangeApi::Clients::Kraken::LimitTrader
                 else
                   ExchangeApi::Clients::Kraken::MarketTrader
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
