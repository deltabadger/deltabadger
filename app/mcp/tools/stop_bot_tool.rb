# frozen_string_literal: true

class StopBotTool < ApplicationMCPTool
  tool_name 'stop_bot'
  description 'Stop a running DCA bot'

  property :bot_id, type: 'number', required: true, description: 'The bot ID'

  def perform
    user = current_user
    bot = user.bots.not_deleted.find_by(id: bot_id.to_i)

    unless bot
      render text: 'Bot not found.'
      return
    end

    unless bot.working?
      render text: "Bot '#{bot.label}' is not running (#{bot.status})."
      return
    end

    bot.set_missed_quote_amount
    if bot.stop
      render text: "Bot '#{bot.label}' stopped."
    else
      render text: "Failed to stop bot '#{bot.label}'."
    end
  end
end
