# frozen_string_literal: true

class ListOpenOrdersTool < ApplicationMCPTool
  tool_name 'list_open_orders'
  description 'List currently open (unfilled) orders across all bots'
  read_only

  property :exchange_name, type: 'string', description: 'Filter by exchange name (optional)'

  def perform
    user = current_user
    orders = user.transactions.submitted.open.order(created_at: :desc)

    if exchange_name.present?
      exchange = Exchange.where('LOWER(name) = ?', exchange_name.downcase).first
      unless exchange
        render text: "Exchange '#{exchange_name}' not found. Available: #{Exchange.where(available: true).pluck(:name).join(', ')}"
        return
      end
      orders = orders.where(exchange: exchange)
    end

    orders = orders.limit(100)

    if orders.empty?
      render text: 'No open orders found.'
      return
    end

    lines = orders.map do |txn|
      date = txn.created_at.strftime('%Y-%m-%d %H:%M')
      pair = "#{txn.base}/#{txn.quote}"
      amount_str = txn.amount ? txn.amount.to_s : 'N/A'
      price_str = txn.price ? "@ #{txn.price}" : ''
      type = txn.order_type&.humanize || 'Unknown'

      "- [#{date}] ##{txn.id} #{txn.side.upcase} #{amount_str} #{pair} #{price_str} (#{type}) | #{txn.exchange.name} | ext: #{txn.external_id}"
    end

    render text: "Open orders (#{lines.size}):\n#{lines.join("\n")}"
  end
end
