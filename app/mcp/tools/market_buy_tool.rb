# frozen_string_literal: true

class MarketBuyTool < ApplicationMCPTool
  tool_name 'market_buy'
  description 'Execute a market buy order on a connected exchange (crypto or stocks via Alpaca)'
  open_world
  destructive

  property :exchange_name, type: 'string', required: true, description: 'Exchange name (e.g., Binance, Kraken, Coinbase, Alpaca)'
  property :base_asset, type: 'string', required: true, description: 'Asset symbol to buy (e.g., BTC, ETH, QQQM, AAPL)'
  property :quote_asset, type: 'string', required: true, description: 'Quote currency symbol (e.g., USD, USDT)'
  property :amount, type: 'number', required: true, description: 'Amount to spend or buy'
  property :amount_type, type: 'string',
                         description: "'quote' (spend in quote currency) or 'base' (buy in base asset). Default: 'quote'"

  def perform
    result = BotApi::Orders::MarketBuy.call(
      user: current_user,
      exchange_name: exchange_name, base_asset: base_asset, quote_asset: quote_asset,
      amount: amount, amount_type: amount_type,
      dry_run: current_user.mcp_dry_run?
    )

    prefix = current_user.mcp_dry_run? ? '[DRY RUN] ' : ''
    return render(text: "#{prefix}#{result.error_message}") unless result.success?

    data = result.data
    currency = data[:amount_type] == 'quote' ? quote_asset.upcase : base_asset.upcase
    render text: "#{prefix}Market buy order placed on #{data[:exchange]}: #{amount} #{currency} of #{data[:pair]}. #{data[:upstream]}"
  end
end
