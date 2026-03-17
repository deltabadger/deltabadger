# frozen_string_literal: true

class CreateBotTool < ApplicationMCPTool
  tool_name 'create_bot'
  description 'Create and start a new DCA bot. One base asset for single-asset, two for dual-asset.'

  property :exchange_name, type: 'string', required: true, description: 'Exchange name (e.g., Binance, Kraken, Coinbase, Alpaca)'
  property :base_asset, type: 'string', required: true, description: 'Asset symbol to buy (e.g., BTC, ETH, QQQM)'
  property :second_base_asset, type: 'string', description: 'Second asset for dual-asset bot (e.g., ETH). Omit for single-asset.'
  property :quote_asset, type: 'string', required: true, description: 'Quote currency to spend (e.g., USD, USDT)'
  property :quote_amount, type: 'number', required: true, description: 'Amount to spend per interval in quote currency'
  property :interval, type: 'string', required: true, description: 'Order interval: hour, day, week, or month'
  property :allocation, type: 'number', description: 'Percentage (0-100) allocated to first base asset in dual-asset bot. Default: 50.'
  property :label, type: 'string', description: 'Custom bot label (optional)'

  def perform
    return render_invalid_interval unless valid_interval?

    exchange = find_exchange
    return unless exchange

    return unless find_api_key(exchange)

    base = find_asset_with_ticker(exchange, base_asset, quote_asset)
    return unless base

    if second_base_asset.present?
      create_dual_asset_bot(exchange, base)
    else
      create_single_asset_bot(exchange, base)
    end
  end

  private

  VALID_INTERVALS = %w[hour day week month].freeze

  def valid_interval?
    VALID_INTERVALS.include?(interval)
  end

  def render_invalid_interval
    render text: "Invalid interval '#{interval}'. Must be one of: #{VALID_INTERVALS.join(', ')}"
  end

  def find_exchange
    exchange = Exchange.where('LOWER(name) = ?', exchange_name.downcase).first
    return exchange if exchange

    render text: "Exchange '#{exchange_name}' not found. Available: #{Exchange.where(available: true).pluck(:name).join(', ')}"
    nil
  end

  def find_api_key(exchange)
    api_key = current_user.api_keys.find_by(exchange: exchange, key_type: :trading, status: :correct)
    return api_key if api_key

    render text: "No valid API key found for #{exchange.name}. Please add an API key in Settings."
    nil
  end

  def find_asset_with_ticker(exchange, base_symbol, quote_symbol)
    ticker = exchange.tickers.available
                     .joins(:base_asset, :quote_asset)
                     .where(assets: { symbol: base_symbol.upcase })
                     .where(quote_assets_tickers: { symbol: quote_symbol.upcase })
                     .first

    unless ticker
      render text: "Trading pair #{base_symbol.upcase}/#{quote_symbol.upcase} not found on #{exchange.name}."
      return nil
    end

    { base_asset_id: ticker.base_asset_id, quote_asset_id: ticker.quote_asset_id }
  end

  def create_single_asset_bot(exchange, asset_ids)
    bot = current_user.bots.new(
      type: 'Bots::DcaSingleAsset',
      exchange: exchange,
      label: effective_label(base_asset.upcase, quote_asset.upcase, exchange),
      settings: {
        'base_asset_id' => asset_ids[:base_asset_id],
        'quote_asset_id' => asset_ids[:quote_asset_id],
        'quote_amount' => quote_amount.to_f,
        'interval' => interval
      }
    )

    save_and_start(bot)
  end

  def create_dual_asset_bot(exchange, first_asset_ids)
    second = find_asset_with_ticker(exchange, second_base_asset, quote_asset)
    return unless second

    if allocation.present? && !allocation.to_f.between?(0, 100)
      render text: "Invalid allocation '#{allocation}'. Must be a percentage between 0 and 100."
      return
    end

    effective_allocation = allocation.present? ? (allocation.to_f / 100) : 0.5

    bot = current_user.bots.new(
      type: 'Bots::DcaDualAsset',
      exchange: exchange,
      label: effective_label("#{base_asset.upcase}+#{second_base_asset.upcase}", quote_asset.upcase, exchange),
      settings: {
        'base0_asset_id' => first_asset_ids[:base_asset_id],
        'base1_asset_id' => second[:base_asset_id],
        'quote_asset_id' => first_asset_ids[:quote_asset_id],
        'quote_amount' => quote_amount.to_f,
        'interval' => interval,
        'allocation0' => effective_allocation
      }
    )

    save_and_start(bot)
  end

  def save_and_start(bot)
    bot.set_missed_quote_amount

    unless bot.valid?
      render text: "Failed to create bot: #{bot.errors.full_messages.join(', ')}"
      return
    end

    if bot.save && bot.start(start_fresh: true)
      pair = format_pair(bot)
      render text: "Bot '#{bot.label}' created and started — #{pair} on #{bot.exchange.name}, " \
                   "#{quote_amount} #{quote_asset.upcase}/#{interval}."
    else
      render text: "Failed to create bot: #{bot.errors.full_messages.join(', ')}"
    end
  end

  def format_pair(bot)
    if bot.dca_dual_asset?
      "#{base_asset.upcase}+#{second_base_asset.upcase}/#{quote_asset.upcase}"
    else
      "#{base_asset.upcase}/#{quote_asset.upcase}"
    end
  end

  def effective_label(pair_str, quote_str, exchange)
    return label if label.present?

    "#{pair_str}/#{quote_str} #{exchange.name}"
  end
end
