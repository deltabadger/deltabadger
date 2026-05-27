# frozen_string_literal: true

module BotApi
  module Orders
    # Lists open (unfilled) orders. Two sources: bot-managed orders in the
    # local DB, and live exchange-side orders that aren't tracked in DB.
    # Results from the exchange are deduplicated against DB external_ids.
    class ListOpen
      DB_LIMIT = 100

      def self.call(user:, exchange_name: nil)
        new(user: user, exchange_name: exchange_name).call
      end

      def initialize(user:, exchange_name: nil)
        @user = user
        @exchange_name = exchange_name
      end

      def call
        if @exchange_name.present?
          @exchange_filter = Lookup.find_exchange(@exchange_name)
          return Lookup.exchange_not_found(@exchange_name) unless @exchange_filter
        end

        db_rows, db_external_ids = local_orders
        exchange_rows = exchange_orders(db_external_ids)
        rows = db_rows + exchange_rows

        Result.success({ count: rows.size, orders: rows })
      end

      private

      def local_orders
        scope = @user.transactions.submitted.open.order(created_at: :desc)
        scope = scope.where(exchange: @exchange_filter) if @exchange_filter
        rows = []
        external_ids = Set.new
        scope.limit(DB_LIMIT).each do |txn|
          external_ids << txn.external_id if txn.external_id.present?
          rows << {
            source: 'db',
            id: txn.id,
            external_id: txn.external_id,
            exchange: txn.exchange.name,
            side: txn.side.to_s,
            order_type: txn.order_type&.to_s,
            pair: "#{txn.base}/#{txn.quote}",
            amount: txn.amount,
            price: txn.price,
            created_at: txn.created_at
          }
        end
        [rows, external_ids]
      end

      def exchange_orders(db_external_ids)
        exchanges_to_query.flat_map do |ex|
          next [] unless ex.respond_to?(:list_open_orders)

          api_key = Lookup.find_api_key(@user, ex)
          next [] unless api_key

          ex.set_client(api_key: api_key)
          result = ex.list_open_orders
          next [] if result.failure?

          map_exchange_rows(ex, result.data, db_external_ids)
        end
      end

      def map_exchange_rows(exchange, data, db_external_ids)
        data.filter_map do |order|
          next if db_external_ids.include?(order[:order_id])

          ticker = order[:ticker]
          {
            source: 'exchange',
            external_id: order[:order_id],
            exchange: exchange.name,
            side: order[:side]&.to_s,
            order_type: order[:order_type]&.to_s,
            pair: ticker ? "#{ticker.base}/#{ticker.quote}" : nil,
            amount: order[:amount],
            price: order[:price]
          }
        end
      end

      def exchanges_to_query
        return [@exchange_filter] if @exchange_filter

        @user.api_keys.where(key_type: :trading, status: :correct)
             .includes(:exchange).map(&:exchange).uniq
      end
    end
  end
end
