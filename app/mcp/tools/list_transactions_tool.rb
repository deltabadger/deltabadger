# frozen_string_literal: true

class ListTransactionsTool < ApplicationMCPTool
  tool_name 'list_transactions'
  description 'List recent transactions (trades) with optional bot filter and limit'
  read_only

  property :bot_id, type: 'number', description: 'Filter by bot ID (optional)'
  property :limit, type: 'number', description: 'Number of transactions to return (default: 20, max: 100)'

  def perform
    result = BotApi::Transactions::List.call(user: current_user, bot_id: bot_id, limit: limit)
    return render(text: result.error_message) unless result.success?

    if result.data[:count].zero?
      render text: 'No transactions found.'
      return
    end

    render text: present(result.data)
  end

  private

  def present(data)
    lines = data[:transactions].map do |row|
      date = row[:created_at].strftime('%Y-%m-%d %H:%M')
      amount_str = row[:amount_exec] ? "#{row[:amount_exec]} #{row[:base]}" : 'N/A'
      price_str = row[:price] ? "@ #{row[:price]} #{row[:quote]}" : ''
      cost_str = row[:quote_amount_exec] ? "(#{row[:quote_amount_exec]} #{row[:quote]})" : ''
      "- [#{date}] #{row[:side].upcase} #{amount_str} #{price_str} #{cost_str} | #{row[:status]}"
    end
    "Transactions (#{data[:count]}):\n#{lines.join("\n")}"
  end
end
