# frozen_string_literal: true

class ExportTransactionsCsvTool < ApplicationMCPTool
  tool_name 'export_transactions_csv'
  description 'Export account transactions as CSV with optional exchange and date filters'
  read_only

  property :exchange_id, type: 'number', description: 'Filter by exchange ID (optional)'
  property :from_date, type: 'string', description: 'Start date in YYYY-MM-DD format (optional)'
  property :to_date, type: 'string', description: 'End date in YYYY-MM-DD format (optional)'

  def perform
    result = BotApi::Transactions::ExportCsv.call(
      user: current_user,
      exchange_id: exchange_id, from_date: from_date, to_date: to_date
    )
    return render(text: result.error_message) unless result.success?

    data = result.data
    if data[:truncated]
      max = BotApi::Transactions::ExportCsv::MAX_ROWS
      warning = "\n\n(Showing first #{max} of #{data[:total]} transactions. Use date filters to narrow the scope.)"
    else
      warning = ''
    end
    render text: "#{data[:csv]}#{warning}"
  end
end
