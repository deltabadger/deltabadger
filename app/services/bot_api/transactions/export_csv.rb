# frozen_string_literal: true

module BotApi
  module Transactions
    # Generates a CSV of account transactions. This is the only REST endpoint
    # that does NOT return the JSON envelope — the controller serves it as
    # `text/csv` with a Content-Disposition attachment header. The service
    # returns a string `csv` (and a `truncated`/`total` flag) so both surfaces
    # (REST and MCP) can present consistently.
    class ExportCsv
      MAX_ROWS = 5000

      def self.call(user:, exchange_id: nil, from_date: nil, to_date: nil)
        new(user: user, exchange_id: exchange_id, from_date: from_date, to_date: to_date).call
      end

      def initialize(user:, exchange_id: nil, from_date: nil, to_date: nil)
        @user = user
        @exchange_id = exchange_id
        @from_date = from_date
        @to_date = to_date
      end

      def call
        if @exchange_id.present?
          exchange = Exchange.find_by(id: @exchange_id.to_i)
          return exchange_not_found unless exchange
        end

        from, to, date_error = parse_dates
        return date_error if date_error

        scope = AccountTransaction.for_user(@user)
        scope = scope.for_exchange(exchange) if exchange
        scope = scope.in_date_range(from, to)

        total = scope.count
        return empty if total.zero?

        rows = scope.order(transacted_at: :desc).limit(MAX_ROWS)
        Result.success({
                         csv: AccountTransaction.to_csv(rows),
                         total: total,
                         returned: [total, MAX_ROWS].min,
                         truncated: total > MAX_ROWS
                       })
      end

      private

      def parse_dates
        from = @from_date.present? ? Date.parse(@from_date.to_s).beginning_of_day : nil
        to = @to_date.present? ? Date.parse(@to_date.to_s).end_of_day : nil
        [from, to, nil]
      rescue ArgumentError, TypeError => e
        [nil, nil, Result.failure(:validation_failed, 'invalid_date',
                                  "Invalid date format. Use YYYY-MM-DD. (#{e.message})")]
      end

      def exchange_not_found
        Result.failure(:not_found, 'exchange_not_found',
                       "Exchange not found. Use 'list_exchanges' to see available exchanges.")
      end

      def empty
        Result.failure(:not_found, 'no_transactions',
                       'No account transactions found matching the filters.')
      end
    end
  end
end
