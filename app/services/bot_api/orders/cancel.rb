# frozen_string_literal: true

module BotApi
  module Orders
    # Cancels an order. Two paths:
    #   - numeric `order_id` → local DB row (bot-managed); calls `transaction.cancel`
    #   - non-numeric `order_id` → exchange-side order; requires `exchange_name`
    #     and calls the exchange API.
    # `dry_run` is honored on both paths via the thread-local flag the legacy
    # MCP wrapper used.
    class Cancel
      def self.call(user:, order_id:, exchange_name: nil, dry_run: false)
        new(user: user, order_id: order_id, exchange_name: exchange_name, dry_run: dry_run).call
      end

      def initialize(user:, order_id:, exchange_name: nil, dry_run: false)
        @user = user
        @order_id = order_id
        @exchange_name = exchange_name
        @dry_run = dry_run
      end

      def call
        return missing_order_id if @order_id.blank?

        if numeric_id?
          local_result = cancel_local
          return local_result if local_result
        end

        cancel_via_exchange
      end

      private

      def numeric_id?
        @order_id.to_s.match?(/\A\d+\z/)
      end

      def cancel_local
        transaction = @user.transactions.submitted.open.find_by(id: @order_id.to_i)
        return nil unless transaction

        upstream = Lookup.with_dry_run(@dry_run) { transaction.cancel }
        if upstream.success?
          Result.success({
                           dry_run: @dry_run,
                           id: transaction.id,
                           exchange: transaction.exchange.name,
                           pair: "#{transaction.base}/#{transaction.quote}",
                           side: transaction.side.to_s,
                           cancelled: true
                         })
        else
          Result.failure(:upstream_failed, 'cancel_failed',
                         "Cancel failed: #{Array(upstream.errors).join(', ')}")
        end
      end

      def cancel_via_exchange
        return exchange_name_required if @exchange_name.blank?

        exchange = Lookup.find_exchange(@exchange_name)
        return Lookup.exchange_not_found(@exchange_name) unless exchange

        api_key = Lookup.find_api_key(@user, exchange)
        return Lookup.api_key_missing(exchange) unless api_key

        exchange.set_client(api_key: api_key)
        upstream = Lookup.with_dry_run(@dry_run) { exchange.cancel_order(order_id: @order_id) }

        if upstream.success?
          Result.success({
                           dry_run: @dry_run,
                           external_id: @order_id,
                           exchange: exchange.name,
                           cancelled: true
                         })
        else
          Result.failure(:upstream_failed, 'cancel_failed',
                         "Cancel failed: #{Array(upstream.errors).join(', ')}")
        end
      end

      def missing_order_id
        Result.failure(:validation_failed, 'missing_required_parameter',
                       'Missing required parameter(s): order_id.')
      end

      def exchange_name_required
        Result.failure(:validation_failed, 'exchange_name_required',
                       'Exchange name is required when cancelling by exchange order ID.')
      end
    end
  end
end
