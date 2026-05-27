# frozen_string_literal: true

class StopBotTool < ApplicationMCPTool
  tool_name 'stop_bot'
  description 'Stop a running DCA bot'

  property :bot_id, type: 'number', required: true, description: 'The bot ID'

  def perform
    result = BotApi::Bots::Stop.call(user: current_user, bot_id: bot_id)
    return render(text: result.error_message) unless result.success?

    render text: "Bot '#{result.data[:label]}' stopped."
  end
end
