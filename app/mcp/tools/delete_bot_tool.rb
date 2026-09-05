# frozen_string_literal: true

class DeleteBotTool < ApplicationMCPTool
  tool_name 'delete_bot'
  description 'Delete a bot (running or not). Its schedule is cancelled; its history is kept but hidden.'

  property :bot_id, type: 'number', required: true, description: 'The bot ID'

  def perform
    result = BotApi::Bots::Delete.call(user: current_user, bot_id: bot_id)
    return render(text: result.error_message) unless result.success?

    render text: "Bot '#{result.data[:label]}' deleted."
  end
end
