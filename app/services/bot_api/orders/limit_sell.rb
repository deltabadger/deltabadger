# frozen_string_literal: true

module BotApi
  module Orders
    class LimitSell
      def self.call(user:, **opts)
        new(user: user, **opts).call
      end

      def initialize(user:, exchange_name: nil, base_asset: nil, quote_asset: nil,
                     amount: nil, price: nil, amount_type: nil, dry_run: false)
        @user = user
        @exchange_name = exchange_name
        @base_asset = base_asset
        @quote_asset = quote_asset
        @amount = amount
        @price = price
        @amount_type = amount_type
        @dry_run = dry_run
      end

      def call
        missing = %i[exchange_name base_asset quote_asset amount price].select do |k|
          instance_variable_get("@#{k}").blank?
        end
        if missing.any?
          return Result.failure(:validation_failed, 'missing_required_parameter',
                                "Missing required parameter(s): #{missing.join(', ')}.")
        end

        exchange = Lookup.find_exchange(@exchange_name)
        return Lookup.exchange_not_found(@exchange_name) unless exchange

        api_key = Lookup.find_api_key(@user, exchange)
        return Lookup.api_key_missing(exchange) unless api_key

        ticker = Lookup.find_ticker(exchange, @base_asset, @quote_asset)
        return Lookup.ticker_not_found(exchange, @base_asset, @quote_asset) unless ticker

        exchange.set_client(api_key: api_key)
        effective_type = (@amount_type.presence || 'base').to_sym
        upstream = Lookup.with_dry_run(@dry_run) do
          exchange.limit_sell(ticker: ticker, amount: @amount, amount_type: effective_type, price: @price)
        end

        if upstream.success?
          Result.success({
                           dry_run: @dry_run,
                           exchange: exchange.name,
                           pair: "#{@base_asset.upcase}/#{@quote_asset.upcase}",
                           side: 'sell',
                           order_type: 'limit',
                           amount: @amount,
                           amount_type: effective_type.to_s,
                           price: @price,
                           upstream: upstream.data
                         }, status: :created)
        else
          Result.failure(:upstream_failed, 'order_failed',
                         "Order failed: #{Array(upstream.errors).join(', ')}")
        end
      end
    end
  end
end
