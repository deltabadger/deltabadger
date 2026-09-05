# frozen_string_literal: true

class LiquidateExitedAssetTool < ApplicationMCPTool
  tool_name 'liquidate_exited_asset'
  description 'Sell, at market, a holding an index or basket bot no longer includes. Irreversible and a ' \
              'taxable disposal. See get_bot_details for exited holdings.'
  open_world
  destructive

  property :bot_id, type: 'number', required: true, description: 'The bot ID'
  property :symbol, type: 'string', required: true, description: 'Symbol of the exited holding to sell (e.g., DOGE)'

  def perform
    result = BotApi::Bots::LiquidateExited.call(user: current_user, bot_id: bot_id, symbol: symbol,
                                                dry_run: current_user.mcp_dry_run?)
    prefix = current_user.mcp_dry_run? ? '[DRY RUN] ' : ''
    return render(text: "#{prefix}#{result.error_message}") unless result.success?

    verb = result.data[:dry_run] ? 'Would sell' : 'Selling'
    tail = result.data[:dry_run] ? 'nothing was queued.' : "queued; check the bot's transactions for the fill."
    render text: "#{prefix}#{verb} #{result.data[:symbol]} from bot '#{result.data[:label]}' — #{tail}"
  end
end
