# frozen_string_literal: true

class StartBotTool < ApplicationMCPTool
  tool_name 'start_bot'
  description 'Start a stopped or newly created DCA bot. Restarting a stopped bot takes the web restart dialog\'s ' \
              'choice as start_fresh.'

  property :bot_id, type: 'number', required: true, description: 'The bot ID'
  property :start_fresh, type: 'boolean',
                         description: 'true (default, as the Start button): skip — start a new schedule now, or at ' \
                                      'the bot\'s starting time, and skip anything missed. false: continue the ' \
                                      'original schedule — wait out the current countdown, or invest what was ' \
                                      'missed while the bot was stopped.'

  def perform
    result = BotApi::Bots::Start.call(user: current_user, bot_id: bot_id, start_fresh: start_fresh)
    return report_error(result.error_message) unless result.success?

    render text: "Bot '#{result.data[:label]}' started successfully."
  end
end
