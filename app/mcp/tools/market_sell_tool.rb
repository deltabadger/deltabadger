# frozen_string_literal: true

class MarketSellTool < ApplicationMCPTool
  tool_name 'market_sell'
  description 'Execute a market sell order on a connected exchange'
  open_world
  destructive

  property :exchange_name, type: 'string', required: true, description: 'Exchange name (e.g., Binance, Kraken, Coinbase)'
  property :base_asset, type: 'string', required: true, description: 'Asset symbol to sell (e.g., BTC, ETH)'
  property :quote_asset, type: 'string', required: true, description: 'Quote currency symbol (e.g., USD, USDT)'
  property :amount, type: 'number', required: true, description: 'Amount to sell'
  property :amount_type, type: 'string',
                         description: "'base' (sell in base asset) or 'quote' (sell worth in quote currency). Default: 'base'"

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
    effective_amount_type = amount_type.present? ? amount_type : 'base'
    result = exchange.market_sell(ticker: ticker, amount: amount, amount_type: effective_amount_type)

    if result.success?
      currency = effective_amount_type == 'base' ? base_asset.upcase : quote_asset.upcase
      pair = "#{base_asset.upcase}/#{quote_asset.upcase}"
      render text: "Market sell order placed on #{exchange.name}: #{amount} #{currency} of #{pair}. #{result.data}"
    else
      render text: "Order failed: #{result.errors.join(', ')}"
    end
  end
end
