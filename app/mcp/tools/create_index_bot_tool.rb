# frozen_string_literal: true

class CreateIndexBotTool < ApplicationMCPTool
  tool_name 'create_index_bot'
  description 'Create and start an index DCA bot that buys the top N assets of an index at market-cap weights. ' \
              'Use list_indices for the index ids.'

  property :exchange_name, type: 'string', required: true, description: 'Exchange name (e.g., Kraken, Binance, Alpaca)'
  property :quote_asset, type: 'string', required: true, description: 'Quote currency to spend (e.g., USD, EUR, USDT)'
  property :quote_amount, type: 'number', required: true, description: 'Amount to spend per interval in quote currency'
  property :interval, type: 'string', required: true, description: 'Order interval: hour, day, week, or month'
  property :index, type: 'string', description: "Index id from list_indices. Default: 'top-coins'"
  property :num_coins, type: 'number',
                       description: 'How many top assets to hold (2-50). Default: 10, or the whole index when it is smaller'
  property :allocation_flattening, type: 'number', description: '0 = pure market-cap weights, 1 = equal weights. Default: 0'
  property :label, type: 'string', description: 'Custom bot label (optional)'
  property :start_at, type: 'string', description: 'Optional ISO8601 datetime for the first buy. Must be in the future.'

  def perform
    result = BotApi::Bots::CreateIndex.call(
      user: current_user, exchange_name: exchange_name, quote_asset: quote_asset, quote_amount: quote_amount,
      interval: interval, index: index, num_coins: num_coins, allocation_flattening: allocation_flattening,
      label: label, start_at: start_at
    )
    return render(text: result.error_message) unless result.success?

    d = result.data
    render text: "Bot '#{d[:label]}' created and started — #{d[:index]} top #{d[:num_coins]} on " \
                 "#{d[:exchange]}, #{d[:quote_amount]} #{d[:quote_asset]}/#{d[:interval]}."
  end
end
