# frozen_string_literal: true

class StopRuleTool < ApplicationMCPTool
  tool_name 'stop_rule'
  description 'Stop an active rule'

  property :rule_id, type: 'number', required: true, description: 'The rule ID'

  def perform
    result = BotApi::Rules::Stop.call(user: current_user, rule_id: rule_id)
    return render(text: result.error_message) unless result.success?

    render text: "Rule ##{result.data[:id]} stopped."
  end
end
