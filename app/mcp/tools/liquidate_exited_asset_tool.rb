# frozen_string_literal: true

class LiquidateExitedAssetTool < ApplicationMCPTool
  tool_name 'liquidate_exited_asset'
  description 'Sell, at market, one holding of an index or basket bot — a current constituent or one that left ' \
              'the composition. Irreversible and a taxable disposal. See get_bot_details for holdings. ' \
              "A sale at a loss on the bot's own lots locks the asset out of buying for the bot's wash-sale window."
  open_world
  destructive

  property :bot_id, type: 'number', required: true, description: 'The bot ID'
  property :symbol, type: 'string', required: true, description: 'Symbol of the holding to sell (e.g., AAPL)'

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
