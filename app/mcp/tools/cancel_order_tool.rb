# frozen_string_literal: true

class CancelOrderTool < ApplicationMCPTool
  tool_name 'cancel_order'
  description 'Cancel an open order by its ID (use list_open_orders to find order IDs)'
  open_world
  destructive

  property :order_id, type: 'string', required: true,
                      description: 'The order ID to cancel — numeric DB ID (bot orders) or exchange order ID (from list_open_orders)'
  property :exchange_name, type: 'string',
                           description: 'Exchange name (required when cancelling by exchange order ID, e.g., Alpaca)'

  def perform
    user = current_user

    # Try local DB first (numeric ID = bot-managed order)
    if order_id.match?(/\A\d+\z/)
      transaction = user.transactions.submitted.open.find_by(id: order_id.to_i)
      if transaction
        result = with_dry_run_if_enabled { transaction.cancel }

        dry_prefix = AppConfig.mcp_dry_run? ? '[DRY RUN] ' : ''
        if result.success?
          pair = "#{transaction.base}/#{transaction.quote}"
          render text: "#{dry_prefix}Order ##{transaction.id} (#{transaction.side.upcase} #{pair} " \
                       "on #{transaction.exchange.name}) cancellation submitted."
        else
          render text: "#{dry_prefix}Cancel failed: #{result.errors.join(', ')}"
        end
        return
      end
    end

    # Exchange order ID — cancel directly via exchange API
    unless exchange_name.present?
      render text: "Exchange name is required when cancelling by exchange order ID. Use: cancel_order(order_id: '...', exchange_name: 'Alpaca')"
      return
    end

    exchange = Exchange.where('LOWER(name) = ?', exchange_name.downcase).first
    unless exchange
      render text: "Exchange '#{exchange_name}' not found. Available: #{Exchange.where(available: true).pluck(:name).join(', ')}"
      return
    end

    api_key = user.api_keys.find_by(exchange: exchange, key_type: :trading, status: :correct)
    unless api_key
      render text: "No valid API key found for #{exchange.name}."
      return
    end

    exchange.set_client(api_key: api_key)

    result = with_dry_run_if_enabled do
      exchange.cancel_order(order_id: order_id)
    end

    dry_prefix = AppConfig.mcp_dry_run? ? '[DRY RUN] ' : ''
    if result.success?
      render text: "#{dry_prefix}Order #{order_id} cancellation submitted on #{exchange.name}."
    else
      render text: "#{dry_prefix}Cancel failed: #{result.errors.join(', ')}"
    end
  end
end
