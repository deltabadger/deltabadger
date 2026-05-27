# frozen_string_literal: true

class UpdateBotSettingsTool < ApplicationMCPTool
  tool_name 'update_bot_settings'
  description 'Update settings on a stopped or newly created bot (quote_amount, label)'

  property :bot_id, type: 'number', required: true, description: 'The bot ID'
  property :quote_amount, type: 'number', description: 'Amount per order in quote currency (optional)'
  property :label, type: 'string', description: 'Bot label (optional)'

  def perform
    result = BotApi::Bots::UpdateSettings.call(
      user: current_user, bot_id: bot_id, quote_amount: quote_amount, label: label
    )
    return render(text: result.error_message) unless result.success?

    render text: "Bot '#{result.data[:label]}' settings updated: #{result.data[:updated].join(', ')}."
  end
end
