# frozen_string_literal: true

module BotApi
  module Orders
    # Shared lookups for the trade services. Each method returns either the
    # resolved record (Exchange / ApiKey / Ticker) or a `BotApi::Result`
    # failure ready to be returned from the caller. Keeps the four trade
    # services from duplicating identical error wiring.
    module Lookup
      module_function

      def find_exchange(name)
        return nil if name.blank?

        Exchange.where('LOWER(name) = ?', name.to_s.downcase).first
      end

      def exchange_not_found(name)
        Result.failure(:not_found, 'exchange_not_found',
                       "Exchange '#{name}' not found. Available: #{Exchange.where(available: true).pluck(:name).join(', ')}")
      end

      def find_api_key(user, exchange)
        user.api_keys.find_by(exchange: exchange, key_type: :trading, status: :correct)
      end

      def api_key_missing(exchange)
        Result.failure(:permission_denied, 'api_key_missing',
                       "No valid API key found for #{exchange.name}. Please add an API key in Settings.")
      end

      def find_ticker(exchange, base_symbol, quote_symbol)
        exchange.tickers
                .joins(:base_asset, :quote_asset)
                .where(assets: { symbol: base_symbol.to_s.upcase })
                .where(quote_assets_tickers: { symbol: quote_symbol.to_s.upcase })
                .first
      end

      def ticker_not_found(exchange, base_symbol, quote_symbol)
        Result.failure(:not_found, 'pair_not_found',
                       "Trading pair #{base_symbol.to_s.upcase}/#{quote_symbol.to_s.upcase} not found on #{exchange.name}.")
      end

      # Wraps a block in the legacy `Thread.current[:force_dry_run]` flag
      # so exchange code paths that consult it behave consistently regardless
      # of whether the call originated from MCP (where dry_run can be on)
      # or REST (where it is always off).
      def with_dry_run(enabled)
        if enabled
          Thread.current[:force_dry_run] = true
          begin
            yield
          ensure
            Thread.current[:force_dry_run] = nil
          end
        else
          yield
        end
      end
    end
  end
end
