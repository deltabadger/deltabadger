# frozen_string_literal: true

class UnarchiveBotTool < ApplicationMCPTool
  tool_name 'unarchive_bot'
  description 'Bring an archived bot back, stopped.'

  property :bot_id, type: 'number', required: true, description: 'The bot ID'

  def perform
    result = BotApi::Bots::Unarchive.call(user: current_user, bot_id: bot_id)
    return render(text: result.error_message) unless result.success?

    render text: "Bot '#{result.data[:label]}' reactivated (stopped)."
  end
end
