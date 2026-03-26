# frozen_string_literal: true

class ExportTransactionsCsvTool < ApplicationMCPTool
  tool_name 'export_transactions_csv'
  description 'Export account transactions as CSV with optional exchange and date filters'
  read_only

  property :exchange_id, type: 'number', description: 'Filter by exchange ID (optional)'
  property :from_date, type: 'string', description: 'Start date in YYYY-MM-DD format (optional)'
  property :to_date, type: 'string', description: 'End date in YYYY-MM-DD format (optional)'

  MAX_ROWS = 5000

  def perform
    scope = AccountTransaction.for_user(current_user)

    if exchange_id.present?
      exchange = Exchange.find_by(id: exchange_id.to_i)
      unless exchange
        render text: "Exchange not found. Use 'list_exchanges' to see available exchanges."
        return
      end
      scope = scope.for_exchange(exchange)
    end

    from = from_date.present? ? Date.parse(from_date).beginning_of_day : nil
    to = to_date.present? ? Date.parse(to_date).end_of_day : nil
    scope = scope.in_date_range(from, to)

    total = scope.count
    scope = scope.order(transacted_at: :desc).limit(MAX_ROWS)

    if total.zero?
      render text: 'No account transactions found matching the filters.'
      return
    end

    csv_data = AccountTransaction.to_csv(scope)
    warning = total > MAX_ROWS ? "\n\n(Showing first #{MAX_ROWS} of #{total} transactions. Use date filters to narrow the scope.)" : ''
    render text: "#{csv_data}#{warning}"
  end
end
