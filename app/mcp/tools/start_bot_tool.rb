# frozen_string_literal: true

class StartBotTool < ApplicationMCPTool
  tool_name 'start_bot'
  description 'Start a stopped or newly created DCA bot'

  property :bot_id, type: 'number', required: true, description: 'The bot ID'

  def perform
    user = current_user
    bot = user.bots.not_deleted.find_by(id: bot_id.to_i)

    unless bot
      render text: 'Bot not found.'
      return
    end

    if bot.working?
      render text: "Bot '#{bot.label}' is already running (#{bot.status})."
      return
    end

    start_fresh = bot.created?
    bot.set_missed_quote_amount
    if bot.start(start_fresh: start_fresh)
      render text: "Bot '#{bot.label}' started successfully."
    else
      render text: "Failed to start bot '#{bot.label}': #{bot.errors.full_messages.join(', ')}"
    end
  end
end
