# frozen_string_literal: true

class StartRuleTool < ApplicationMCPTool
  tool_name 'start_rule'
  description 'Start a stopped rule (e.g., withdrawal rule)'

  property :rule_id, type: 'number', required: true, description: 'The rule ID'

  def perform
    result = BotApi::Rules::Start.call(user: current_user, rule_id: rule_id)
    return render(text: result.error_message) unless result.success?

    render text: "Rule ##{result.data[:id]} started."
  end
end
