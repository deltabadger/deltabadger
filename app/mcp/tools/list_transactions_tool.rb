# frozen_string_literal: true

class ListTransactionsTool < ApplicationMCPTool
  tool_name 'list_transactions'
  description 'List recent transactions (trades) with optional bot filter and limit'
  read_only

  property :bot_id, type: 'number', description: 'Filter by bot ID (optional)'
  property :limit, type: 'number', description: 'Number of transactions to return (default: 20, max: 100)'

  def perform
    user = current_user
    max_limit = [limit&.to_i || 20, 100].min
    max_limit = 20 if max_limit <= 0

    transactions = user.transactions.order(created_at: :desc)

    if bot_id.present?
      bot = user.bots.not_deleted.find_by(id: bot_id.to_i)
      unless bot
        render text: 'Bot not found.'
        return
      end
      transactions = transactions.where(bot_id: bot.id)
    end

    transactions = transactions.limit(max_limit)

    if transactions.empty?
      render text: 'No transactions found.'
      return
    end

    lines = transactions.map do |txn|
      date = txn.created_at.strftime('%Y-%m-%d %H:%M')
      amount_str = txn.amount_exec ? "#{txn.amount_exec} #{txn.base}" : 'N/A'
      price_str = txn.price ? "@ #{txn.price} #{txn.quote}" : ''
      cost_str = txn.quote_amount_exec ? "(#{txn.quote_amount_exec} #{txn.quote})" : ''

      "- [#{date}] #{txn.side.upcase} #{amount_str} #{price_str} #{cost_str} | #{txn.status}"
    end

    render text: "Transactions (#{lines.size}):\n#{lines.join("\n")}"
  end
end
