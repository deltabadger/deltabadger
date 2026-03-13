# frozen_string_literal: true

class UpdateBotSettingsTool < ApplicationMCPTool
  tool_name 'update_bot_settings'
  description 'Update settings on a stopped or newly created bot (quote_amount, label)'

  property :bot_id, type: 'number', required: true, description: 'The bot ID'
  property :quote_amount, type: 'number', description: 'Amount per order in quote currency (optional)'
  property :label, type: 'string', description: 'Bot label (optional)'

  def perform
    user = current_user
    bot = user.bots.not_deleted.find_by(id: bot_id.to_i)

    unless bot
      render text: 'Bot not found.'
      return
    end

    if bot.working?
      render text: "Bot must be stopped before updating settings. Current status: #{bot.status}."
      return
    end

    updates = {}
    updates[:quote_amount] = quote_amount if quote_amount.present?
    updates[:label] = label if label.present?

    if updates.empty?
      render text: 'No settings provided to update.'
      return
    end

    bot.quote_amount = updates[:quote_amount] if updates[:quote_amount]
    bot.label = updates[:label] if updates[:label]

    bot.set_missed_quote_amount
    if bot.save
      render text: "Bot '#{bot.label}' settings updated: #{updates.keys.join(', ')}."
    else
      render text: "Failed to update bot: #{bot.errors.full_messages.join(', ')}"
    end
  end
end
