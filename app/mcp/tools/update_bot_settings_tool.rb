# frozen_string_literal: true

class UpdateBotSettingsTool < ApplicationMCPTool
  tool_name 'update_bot_settings'
  description 'Update settings on a stopped or newly created bot: amount and label on any bot, ' \
              'the index knobs on an index bot, the weights on a basket bot'

  property :bot_id, type: 'number', required: true, description: 'The bot ID'
  property :quote_amount, type: 'number', description: 'Amount per order in quote currency (optional)'
  property :label, type: 'string', description: 'Bot label (optional)'
  property :num_coins, type: 'number', description: 'Index bots: how many top assets to hold (2-50)'
  property :allocation_flattening, type: 'number', description: 'Index bots: 0 = market-cap weights, 1 = equal'
  property :allocations, type: 'string',
                         description: "Basket bots: new weights for every current asset, e.g. 'BTC:70,ETH:30' (sum 100)"

  def perform
    result = BotApi::Bots::UpdateSettings.call(
      user: current_user, bot_id: bot_id, quote_amount: quote_amount, label: label,
      num_coins: num_coins, allocation_flattening: allocation_flattening, allocations: allocations
    )
    return render(text: result.error_message) unless result.success?

    render text: "Bot '#{result.data[:label]}' settings updated: #{result.data[:updated].join(', ')}."
  end
end
