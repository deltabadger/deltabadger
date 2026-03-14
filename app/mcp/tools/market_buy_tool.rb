# frozen_string_literal: true

class MarketBuyTool < ApplicationMCPTool
  tool_name 'market_buy'
  description 'Execute a market buy order on a connected exchange'
  open_world
  destructive

  property :exchange_name, type: 'string', required: true, description: 'Exchange name (e.g., Binance, Kraken, Coinbase)'
  property :base_asset, type: 'string', required: true, description: 'Asset symbol to buy (e.g., BTC, ETH)'
  property :quote_asset, type: 'string', required: true, description: 'Quote currency symbol (e.g., USD, USDT)'
  property :amount, type: 'number', required: true, description: 'Amount to spend or buy'
  property :amount_type, type: 'string',
                         description: "'quote' (spend in quote currency) or 'base' (buy in base asset). Default: 'quote'"

  def perform
    exchange = Exchange.where('LOWER(name) = ?', exchange_name.downcase).first
    unless exchange
      render text: "Exchange '#{exchange_name}' not found. Available: #{Exchange.where(available: true).pluck(:name).join(', ')}"
      return
    end

    user = current_user
    api_key = user.api_keys.find_by(exchange: exchange, key_type: :trading, status: :correct)
    unless api_key
      render text: "No valid API key found for #{exchange.name}. Please add an API key in Settings."
      return
    end

    ticker = exchange.tickers.joins(:base_asset, :quote_asset)
                     .where(assets: { symbol: base_asset.upcase })
                     .where(quote_assets_tickers: { symbol: quote_asset.upcase })
                     .first
    unless ticker
      render text: "Trading pair #{base_asset.upcase}/#{quote_asset.upcase} not found on #{exchange.name}."
      return
    end

    exchange.set_client(api_key: api_key)
    effective_amount_type = amount_type.present? ? amount_type : 'quote'

    result = with_dry_run_if_enabled do
      exchange.market_buy(ticker: ticker, amount: amount, amount_type: effective_amount_type)
    end

    dry_prefix = AppConfig.mcp_dry_run? ? '[DRY RUN] ' : ''
    if result.success?
      currency = effective_amount_type == 'quote' ? quote_asset.upcase : base_asset.upcase
      pair = "#{base_asset.upcase}/#{quote_asset.upcase}"
      render text: "#{dry_prefix}Market buy order placed on #{exchange.name}: #{amount} #{currency} of #{pair}. #{result.data}"
    else
      render text: "#{dry_prefix}Order failed: #{result.errors.join(', ')}"
    end
  end
end
