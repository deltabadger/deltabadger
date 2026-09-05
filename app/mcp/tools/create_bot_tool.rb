# frozen_string_literal: true

class CreateBotTool < ApplicationMCPTool
  tool_name 'create_bot'
  description 'Create and start a DCA bot: one base asset, or a basket of 2-20 with weights.'

  property :exchange_name, type: 'string', required: true, description: 'Exchange name (e.g., Binance, Kraken, Coinbase, Alpaca)'
  property :base_asset, type: 'string', description: 'Asset symbol to buy (e.g., BTC, ETH, QQQM). Required unless assets is given.'
  property :assets, type: 'string',
                    description: 'Basket: comma-separated symbols with optional percent weights, ' \
                                 "e.g. 'BTC:60,ETH:40' or 'BTC,ETH,SOL' (equal split). Replaces base_asset."
  property :weighting, type: 'string',
                       description: "'manual' (default) or 'market_cap' (weights derived from market cap; given weights ignored)"
  property :second_base_asset, type: 'string',
                               description: 'Second asset for a two-asset portfolio bot (legacy — prefer assets).'
  property :quote_asset, type: 'string', required: true, description: 'Quote currency to spend (e.g., USD, USDT)'
  property :quote_amount, type: 'number', required: true, description: 'Amount to spend per interval in quote currency'
  property :interval, type: 'string', required: true, description: 'Order interval: hour, day, week, or month'
  property :allocation, type: 'number',
                        description: 'Percentage (0-100) for the first base asset of a two-asset bot ' \
                                     '(legacy — prefer assets). Default: 50.'
  property :label, type: 'string', description: 'Custom bot label (optional)'
  property :start_at, type: 'string',
                      description: 'Optional ISO8601 datetime to schedule the first buy ' \
                                   '(e.g. 2026-06-15T09:30:00Z, or 2026-06-15T09:30 in the account ' \
                                   'time zone). Must be in the future. Omit to start immediately.'

  def perform
    result = BotApi::Bots::Create.call(
      user: current_user,
      exchange_name: exchange_name,
      base_asset: base_asset,
      second_base_asset: second_base_asset,
      assets: assets,
      weighting: weighting,
      quote_asset: quote_asset,
      quote_amount: quote_amount,
      interval: interval,
      allocation: allocation,
      label: label,
      start_at: start_at
    )

    return render(text: result.error_message) unless result.success?

    d = result.data
    pair_summary = "#{d[:pair]} on #{d[:exchange]}, #{quote_amount} #{quote_asset.to_s.upcase}/#{interval}."
    text = if start_at.present?
             "Bot '#{d[:label]}' created — scheduled to start #{d[:started_at]} — #{pair_summary}"
           else
             "Bot '#{d[:label]}' created and started — #{pair_summary}"
           end
    render text: text
  end
end
