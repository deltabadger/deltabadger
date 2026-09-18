# frozen_string_literal: true

class MarketSellTool < ApplicationMCPTool
  tool_name 'market_sell'
  description 'Execute a market sell order on a connected exchange (crypto or stocks via Alpaca)'
  open_world
  destructive

  property :bot_id, type: 'number',
                    description: 'Optional. A running signal bot to place the order through: it is recorded on that ' \
                                 "bot's page and the pair comes from the bot, so the three pair fields can be left out"
  property :exchange_name, type: 'string',
                           description: 'Exchange name (e.g., Binance, Kraken, Coinbase, Alpaca). Required without bot_id'
  property :base_asset, type: 'string', description: 'Asset symbol to sell (e.g., BTC, ETH, QQQM, AAPL). Required without bot_id'
  property :quote_asset, type: 'string', description: 'Quote currency symbol (e.g., USD, USDT). Required without bot_id'
  property :amount, type: 'number', required: true, description: 'Amount to sell or receive'
  property :amount_type, type: 'string',
                         description: "'base' (sell in base asset) or 'quote' (receive in quote currency). Default: 'base'"

  def perform
    result = BotApi::Orders::MarketSell.call(
      user: current_user, bot_id: bot_id,
      exchange_name: exchange_name, base_asset: base_asset, quote_asset: quote_asset,
      amount: amount, amount_type: amount_type,
      dry_run: current_user.mcp_dry_run?
    )

    prefix = current_user.mcp_dry_run? ? '[DRY RUN] ' : ''
    return render(text: "#{prefix}#{result.error_message}") unless result.success?

    data = result.data
    base, quote = data[:pair].split('/')
    currency = data[:amount_type] == 'base' ? base : quote
    render text: "#{prefix}Market sell order #{placed(data)}: #{amount} #{currency} of #{data[:pair]}. #{detail(data)}"
  end

  private

  def placed(data)
    data[:bot_id] ? "placed by bot #{data[:bot_id]} on #{data[:exchange]}" : "placed on #{data[:exchange]}"
  end

  def detail(data)
    return data[:upstream].to_s unless data[:bot_id]
    return 'Nothing was placed or recorded.' if data[:dry_run]

    "Order #{data[:order_id]}, transaction #{data[:transaction_id]}."
  end
end
