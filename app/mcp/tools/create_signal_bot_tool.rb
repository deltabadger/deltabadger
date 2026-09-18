# frozen_string_literal: true

class CreateSignalBotTool < ApplicationMCPTool
  tool_name 'create_signal_bot'
  description 'Create and start a signal bot: a bot with no schedule that trades one pair when told to. ' \
              'Place orders through it with market_buy / market_sell and its bot_id; they are recorded on ' \
              "the bot's page. Webhook rules can be added to it in the app."

  property :exchange_name, type: 'string', required: true, description: 'Exchange name (e.g., Kraken, Binance, Alpaca)'
  property :base_asset, type: 'string', required: true, description: 'Asset the bot trades (e.g., BTC, ETH, AAPL)'
  property :quote_asset, type: 'string', required: true, description: 'Quote currency it trades against (e.g., USD, EUR, USDT)'
  property :label, type: 'string', description: 'Custom bot label (optional)'

  def perform
    result = BotApi::Bots::CreateSignal.call(
      user: current_user, exchange_name: exchange_name, base_asset: base_asset, quote_asset: quote_asset, label: label
    )
    return render(text: result.error_message) unless result.success?

    d = result.data
    render text: "Signal bot '#{d[:label]}' created and started — #{d[:pair]} on #{d[:exchange]}. " \
                 "Place orders through it with market_buy / market_sell and bot_id #{d[:id]}."
  end
end
