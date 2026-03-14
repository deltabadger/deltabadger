# frozen_string_literal: true

class CancelOrderTool < ApplicationMCPTool
  tool_name 'cancel_order'
  description 'Cancel an open order by its ID (use list_open_orders to find order IDs)'
  open_world
  destructive

  property :order_id, type: 'number', required: true, description: 'The order ID to cancel (from list_open_orders)'

  def perform
    user = current_user
    transaction = user.transactions.submitted.open.find_by(id: order_id.to_i)

    unless transaction
      render text: "Open order ##{order_id} not found. Use list_open_orders to see available orders."
      return
    end

    result = with_dry_run_if_enabled do
      transaction.cancel
    end

    dry_prefix = AppConfig.mcp_dry_run? ? '[DRY RUN] ' : ''
    if result.success?
      pair = "#{transaction.base}/#{transaction.quote}"
      render text: "#{dry_prefix}Order ##{transaction.id} (#{transaction.side.upcase} #{pair} " \
                   "on #{transaction.exchange.name}) cancellation submitted."
    else
      render text: "#{dry_prefix}Cancel failed: #{result.errors.join(', ')}"
    end
  end
end
