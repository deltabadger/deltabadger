# frozen_string_literal: true

class ArchiveBotTool < ApplicationMCPTool
  tool_name 'archive_bot'
  description 'Archive a bot: it stops, leaves the dashboard, keeps its history. Reactivate with unarchive_bot.'

  property :bot_id, type: 'number', required: true, description: 'The bot ID'

  def perform
    result = BotApi::Bots::Archive.call(user: current_user, bot_id: bot_id)
    return render(text: result.error_message) unless result.success?

    render text: "Bot '#{result.data[:label]}' archived."
  end
end
