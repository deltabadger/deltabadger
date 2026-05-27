# frozen_string_literal: true

class ListAccountTransactionsTool < ApplicationMCPTool
  tool_name 'list_account_transactions'
  description 'List account transactions (tracker) with optional filters for exchange, date range, and entry type'
  read_only

  property :exchange_id, type: 'number', description: 'Filter by exchange ID (optional)'
  property :from_date, type: 'string', description: 'Start date in YYYY-MM-DD format (optional)'
  property :to_date, type: 'string', description: 'End date in YYYY-MM-DD format (optional)'
  property :entry_type, type: 'string',
                        description: 'Filter by type: buy, sell, swap_in, swap_out, deposit, withdrawal, ' \
                                     'staking_reward, lending_interest, airdrop, mining, fee, other_income, lost (optional)'
  property :limit, type: 'number', description: 'Number of transactions to return (default: 50, max: 200)'

  def perform
    result = BotApi::Transactions::ListAccount.call(
      user: current_user,
      exchange_id: exchange_id, from_date: from_date, to_date: to_date,
      entry_type: entry_type, limit: limit
    )
    return render(text: result.error_message) unless result.success?

    if result.data[:count].zero?
      render text: 'No account transactions found.'
      return
    end

    render text: present(result.data)
  end

  private

  def present(data)
    lines = data[:transactions].map do |row|
      date = row[:transacted_at].strftime('%Y-%m-%d %H:%M')
      base = "#{row[:base_amount]} #{row[:base_currency]}"
      quote = row[:quote_amount].present? ? " / #{row[:quote_amount]} #{row[:quote_currency]}" : ''
      fee = row[:fee_amount].present? ? " | Fee: #{row[:fee_amount]} #{row[:fee_currency]}" : ''
      "- [#{date}] #{row[:entry_type].upcase} #{base}#{quote}#{fee} | #{row[:exchange] || 'N/A'}"
    end
    "Account transactions (#{data[:count]}):\n#{lines.join("\n")}"
  end
end
