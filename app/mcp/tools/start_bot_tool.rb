# frozen_string_literal: true

class StartBotTool < ApplicationMCPTool
  tool_name 'start_bot'
  description 'Start a stopped or newly created DCA bot'

  property :bot_id, type: 'number', required: true, description: 'The bot ID'

  def perform
    result = BotApi::Bots::Start.call(user: current_user, bot_id: bot_id)
    return render(text: result.error_message) unless result.success?

    render text: "Bot '#{result.data[:label]}' started successfully."
  end
end
