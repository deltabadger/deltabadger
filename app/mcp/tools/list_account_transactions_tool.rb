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
    max_limit = [limit&.to_i || 50, 200].min
    max_limit = 50 if max_limit <= 0

    scope = AccountTransaction.for_user(current_user).by_date

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

    scope = scope.where(entry_type: entry_type) if entry_type.present?
    scope = scope.limit(max_limit)

    transactions = scope.includes(:exchange)

    if transactions.empty?
      render text: 'No account transactions found.'
      return
    end

    lines = transactions.map do |tx|
      date = tx.transacted_at.strftime('%Y-%m-%d %H:%M')
      base = "#{tx.base_amount} #{tx.base_currency}"
      quote = tx.quote_amount.present? ? " / #{tx.quote_amount} #{tx.quote_currency}" : ''
      fee = tx.fee_amount.present? ? " | Fee: #{tx.fee_amount} #{tx.fee_currency}" : ''
      exchange_name = tx.exchange&.name || 'N/A'

      "- [#{date}] #{tx.entry_type.upcase} #{base}#{quote}#{fee} | #{exchange_name}"
    end

    render text: "Account transactions (#{lines.size}):\n#{lines.join("\n")}"
  end
end
