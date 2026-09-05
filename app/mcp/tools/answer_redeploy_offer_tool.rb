# frozen_string_literal: true

class AnswerRedeployOfferTool < ApplicationMCPTool
  tool_name 'answer_redeploy_offer'
  description 'Answer an index or basket bot\'s "Redeploy?" offer: accept buys the idle sale proceeds back ' \
              'into the composition at target weights; decline takes them off the table for good.'
  open_world
  destructive

  property :bot_id, type: 'number', required: true, description: 'The bot ID'
  property :accept, type: 'boolean', required: true, description: 'true to redeploy, false to decline'

  def perform
    result = BotApi::Bots::AnswerRedeploy.call(user: current_user, bot_id: bot_id, accept: accept,
                                               dry_run: current_user.mcp_dry_run?)
    prefix = current_user.mcp_dry_run? ? '[DRY RUN] ' : ''
    return render(text: "#{prefix}#{result.error_message}") unless result.success?

    verb = if result.data[:dry_run]
             result.data[:accepted] ? 'Would redeploy' : 'Would decline redeploying'
           else
             result.data[:accepted] ? 'Redeploying' : 'Declined redeploying'
           end
    tail = result.data[:dry_run] ? 'nothing was queued.' : 'queued.'
    render text: "#{prefix}#{verb} #{result.data[:offer]} for bot '#{result.data[:label]}' — #{tail}"
  end
end
