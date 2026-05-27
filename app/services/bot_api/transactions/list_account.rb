# frozen_string_literal: true

module BotApi
  module Transactions
    # Lists ledger-style account transactions (deposits, withdrawals, swaps,
    # rewards, etc.) — the tracker view, distinct from per-bot trades above.
    class ListAccount
      DEFAULT_LIMIT = 50
      MAX_LIMIT = 200

      def self.call(user:, exchange_id: nil, from_date: nil, to_date: nil, entry_type: nil, limit: nil)
        new(user: user, exchange_id: exchange_id, from_date: from_date,
            to_date: to_date, entry_type: entry_type, limit: limit).call
      end

      def initialize(user:, exchange_id:, from_date:, to_date:, entry_type:, limit:)
        @user = user
        @exchange_id = exchange_id
        @from_date = from_date
        @to_date = to_date
        @entry_type = entry_type
        @limit = limit
      end

      def call
        exchange = nil
        if @exchange_id.present?
          exchange = Exchange.find_by(id: @exchange_id.to_i)
          unless exchange
            return Result.failure(:not_found, 'exchange_not_found',
                                  "Exchange not found. Use 'list_exchanges' to see available exchanges.")
          end
        end

        from, to, parse_error = parse_dates
        return parse_error if parse_error

        scope = AccountTransaction.for_user(@user).by_date
        scope = scope.for_exchange(exchange) if exchange
        scope = scope.in_date_range(from, to)
        scope = scope.where(entry_type: @entry_type) if @entry_type.present?

        rows = scope.includes(:exchange).limit(clamp_limit(@limit)).map { |txn| row_for(txn) }
        Result.success({ count: rows.size, transactions: rows })
      end

      private

      def clamp_limit(raw)
        value = raw&.to_i || DEFAULT_LIMIT
        return DEFAULT_LIMIT if value <= 0

        [value, MAX_LIMIT].min
      end

      def parse_dates
        from = @from_date.present? ? Date.parse(@from_date.to_s).beginning_of_day : nil
        to = @to_date.present? ? Date.parse(@to_date.to_s).end_of_day : nil
        [from, to, nil]
      rescue ArgumentError, TypeError => e
        [nil, nil, Result.failure(:validation_failed, 'invalid_date',
                                  "Invalid date format. Use YYYY-MM-DD. (#{e.message})")]
      end

      def row_for(txn)
        {
          id: txn.id,
          transacted_at: txn.transacted_at,
          entry_type: txn.entry_type.to_s,
          base_amount: txn.base_amount,
          base_currency: txn.base_currency,
          quote_amount: txn.quote_amount,
          quote_currency: txn.quote_currency,
          fee_amount: txn.fee_amount,
          fee_currency: txn.fee_currency,
          exchange: txn.exchange&.name
        }
      end
    end
  end
end
