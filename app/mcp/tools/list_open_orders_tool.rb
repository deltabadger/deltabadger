# frozen_string_literal: true

class ListOpenOrdersTool < ApplicationMCPTool
  tool_name 'list_open_orders'
  description 'List currently open (unfilled) orders across all connected exchanges'
  read_only

  property :exchange_name, type: 'string', description: 'Filter by exchange name (optional)'

  def perform
    user = current_user

    exchange = nil
    if exchange_name.present?
      exchange = Exchange.where('LOWER(name) = ?', exchange_name.downcase).first
      unless exchange
        render text: "Exchange '#{exchange_name}' not found. Available: #{Exchange.where(available: true).pluck(:name).join(', ')}"
        return
      end
    end

    lines = []

    # Bot-managed open orders from local DB
    db_orders = user.transactions.submitted.open.order(created_at: :desc)
    db_orders = db_orders.where(exchange: exchange) if exchange

    # Track external IDs from DB orders to avoid duplicates
    db_external_ids = Set.new

    db_orders.limit(100).each do |txn|
      db_external_ids << txn.external_id if txn.external_id.present?
      date = txn.created_at.strftime('%Y-%m-%d %H:%M')
      pair = "#{txn.base}/#{txn.quote}"
      amount_str = txn.amount ? txn.amount.to_s : 'N/A'
      price_str = txn.price ? "@ #{txn.price}" : ''
      type = txn.order_type&.humanize || 'Unknown'

      lines << "- [#{date}] ##{txn.id} #{txn.side.upcase} #{amount_str} #{pair} " \
               "#{price_str} (#{type}) | #{txn.exchange.name} | ext: #{txn.external_id}"
    end

    # Query exchanges directly for orders not tracked in DB
    exchanges_to_query = if exchange
                           [exchange]
                         else
                           user.api_keys.where(key_type: :trading, status: :correct).includes(:exchange).map(&:exchange).uniq
                         end

    exchanges_to_query.each do |ex|
      next unless ex.respond_to?(:list_open_orders)

      api_key = user.api_keys.find_by(exchange: ex, key_type: :trading, status: :correct)
      next unless api_key

      ex.set_client(api_key: api_key)
      result = ex.list_open_orders
      next if result.failure?

      result.data.each do |order|
        next if db_external_ids.include?(order[:order_id])

        ticker = order[:ticker]
        pair = ticker ? "#{ticker.base}/#{ticker.quote}" : order[:order_id]
        amount_str = order[:amount] ? order[:amount].to_s : 'N/A'
        price_str = order[:price] ? "@ #{order[:price]}" : ''
        type = order[:order_type] == :limit_order ? 'Limit order' : 'Market order'
        side = order[:side]&.to_s&.upcase || '?'

        lines << "- #{side} #{amount_str} #{pair} #{price_str} (#{type}) | #{ex.name} | ext: #{order[:order_id]}"
      end
    end

    if lines.empty?
      render text: 'No open orders found.'
      return
    end

    render text: "Open orders (#{lines.size}):\n#{lines.join("\n")}"
  end
end
