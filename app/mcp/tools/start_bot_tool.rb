# frozen_string_literal: true

class StartBotTool < ApplicationMCPTool
  tool_name 'start_bot'
  description 'Start a stopped or newly created DCA bot. A bot that can sell is refused until the wash-sale ' \
              'question is answered in the web app.'

  property :bot_id, type: 'number', required: true, description: 'The bot ID'
  property :start_fresh, type: 'boolean',
                         description: 'Omitted or true: resets the schedule and can place an order immediately, unless ' \
                                      'a future starting time applies. false: keeps the schedule, but can invest ' \
                                      'amounts missed while the bot was stopped.'

  def perform
    result = BotApi::Bots::Start.call(user: current_user, bot_id: bot_id, start_fresh: start_fresh)
    return report_error(result.error_message) unless result.success?

    render text: "Bot '#{result.data[:label]}' started successfully."
  end
end
