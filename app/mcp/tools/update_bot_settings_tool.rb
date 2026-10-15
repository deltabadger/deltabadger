# frozen_string_literal: true

class UpdateBotSettingsTool < ApplicationMCPTool
  tool_name 'update_bot_settings'
  description "Change a stopped bot's settings, as its settings page does: amount, interval, label, exchange; " \
              'index count and flattening; basket weights, members and weighting; selling amounts; Smart Intervals; ' \
              'limit orders; starting time; spending and selling caps; price, price-drop, moving-average and RSI ' \
              'conditions; rebalancing. Only what you pass changes; each field says which bot types have it.'

  property :bot_id, type: 'number', required: true, description: 'The bot ID'
  # A nullable field's type is ["number", "null"]; ActionMCP then keeps it as a string attribute, which
  # is never read: values come from the arguments as sent (#sent).
  BotApi::Bots::SettingsFields::FIELDS.each { |field| property field.name, **BotApi::Bots::SettingsFields.schema(field) }

  def perform
    result = BotApi::Bots::UpdateSettings.call(user: current_user, bot_id: bot_id, **sent)
    return report_error(result.error_message) unless result.success?

    render text: "Bot '#{result.data[:label]}' settings updated: #{result.data[:updated].join(', ')}."
  end

  private

  # Only the fields the caller sent, as sent: an omitted field must stay unchanged and a null must
  # clear, and the cast attributes cannot tell those apart.
  def sent
    sent = defined?(@_wire_arguments) && @_wire_arguments ? @_wire_arguments.to_h.stringify_keys : attributes.compact
    sent.slice(*BotApi::Bots::SettingsFields::NAMES)
  end
end
