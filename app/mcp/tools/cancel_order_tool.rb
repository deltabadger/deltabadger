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
    result = BotApi::Orders::Cancel.call(
      user: current_user, order_id: order_id, exchange_name: exchange_name,
      dry_run: current_user.mcp_dry_run?
    )

    prefix = current_user.mcp_dry_run? ? '[DRY RUN] ' : ''

    unless result.success?
      # The legacy tool surfaced a verbose usage hint for the "missing
      # exchange_name" case. Restore that at the MCP boundary so chat
      # clients get the friendlier copy without REST having to carry it.
      msg = result.error_message
      if result.error_code == 'exchange_name_required'
        msg = "Exchange name is required when cancelling by exchange order ID. Use: cancel_order(order_id: '...', exchange_name: 'Alpaca')"
      end
      render text: "#{prefix}#{msg}"
      return
    end

    data = result.data
    if data[:id]
      render text: "#{prefix}Order ##{data[:id]} (#{data[:side].upcase} #{data[:pair]} " \
                   "on #{data[:exchange]}) cancellation submitted."
    else
      render text: "#{prefix}Order #{data[:external_id]} cancellation submitted on #{data[:exchange]}."
    end
  end
end
