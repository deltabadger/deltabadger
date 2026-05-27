# frozen_string_literal: true

module BotApi
  module Exchanges
    # Fetches live balances from a single exchange. Per-exchange shape matches
    # the existing MCP tool — the REST surface mounts this as
    # GET /api/v1/exchanges/:id/balances.
    class Balances
      def self.call(user:, exchange_id: nil, exchange_name: nil)
        new(user: user, exchange_id: exchange_id, exchange_name: exchange_name).call
      end

      def initialize(user:, exchange_id: nil, exchange_name: nil)
        @user = user
        @exchange_id = exchange_id
        @exchange_name = exchange_name
      end

      def call
        exchange = resolve_exchange
        return exchange_not_found unless exchange

        api_key = @user.api_keys.find_by(exchange: exchange, key_type: :trading, status: :correct)
        return api_key_missing(exchange) unless api_key

        exchange.set_client(api_key: api_key)
        upstream = exchange.get_balances
        return upstream_failed(exchange, upstream.errors) unless upstream.success?

        rows = build_balances(upstream.data)
        Result.success({ exchange: exchange.name, count: rows.size, balances: rows })
      end

      private

      def resolve_exchange
        return Exchange.find_by(id: @exchange_id.to_i) if @exchange_id.present?
        return Exchange.where('LOWER(name) = ?', @exchange_name.to_s.downcase).first if @exchange_name.present?

        nil
      end

      def build_balances(raw)
        Array(raw).filter_map do |asset_id, balance|
          free = balance[:free].to_f
          locked = balance[:locked].to_f
          next if free.zero? && locked.zero?

          asset = Asset.find_by(id: asset_id)
          {
            asset_id: asset_id,
            symbol: asset&.symbol || "Unknown(#{asset_id})",
            free: free,
            locked: locked
          }
        end
      end

      def exchange_not_found
        identifier = @exchange_name.presence || @exchange_id
        Result.failure(:not_found, 'exchange_not_found',
                       "Exchange '#{identifier}' not found. Available exchanges: #{Exchange.where(available: true).pluck(:name).join(', ')}")
      end

      def api_key_missing(exchange)
        Result.failure(:permission_denied, 'api_key_missing',
                       "No valid API key found for #{exchange.name}. Please add an API key in Settings.")
      end

      def upstream_failed(exchange, errors)
        Result.failure(:upstream_failed, 'balances_fetch_failed',
                       "Failed to fetch balances from #{exchange.name}: #{Array(errors).join(', ')}")
      end
    end
  end
end
