# frozen_string_literal: true

class MarketSellTool < ApplicationMCPTool
  tool_name 'market_sell'
  description 'Execute a market sell order on a connected exchange (crypto or stocks via Alpaca)'
  open_world
  destructive

  property :exchange_name, type: 'string', required: true, description: 'Exchange name (e.g., Binance, Kraken, Coinbase, Alpaca)'
  property :base_asset, type: 'string', required: true, description: 'Asset symbol to sell (e.g., BTC, ETH, QQQM, AAPL)'
  property :quote_asset, type: 'string', required: true, description: 'Quote currency symbol (e.g., USD, USDT)'
  property :amount, type: 'number', required: true, description: 'Amount to sell or receive'
  property :amount_type, type: 'string',
                         description: "'base' (sell in base asset) or 'quote' (receive in quote currency). Default: 'base'"

  def perform
    result = BotApi::Orders::MarketSell.call(
      user: current_user,
      exchange_name: exchange_name, base_asset: base_asset, quote_asset: quote_asset,
      amount: amount, amount_type: amount_type,
      dry_run: current_user.mcp_dry_run?
    )

    prefix = current_user.mcp_dry_run? ? '[DRY RUN] ' : ''
    return render(text: "#{prefix}#{result.error_message}") unless result.success?

    data = result.data
    currency = data[:amount_type] == 'base' ? base_asset.upcase : quote_asset.upcase
    render text: "#{prefix}Market sell order placed on #{data[:exchange]}: #{amount} #{currency} of #{data[:pair]}. #{data[:upstream]}"
  end
end
