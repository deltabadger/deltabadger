# frozen_string_literal: true

class DeleteRuleTool < ApplicationMCPTool
  tool_name 'delete_rule'
  description 'Delete a stopped withdrawal rule'

  property :rule_id, type: 'number', required: true, description: 'The rule ID'

  def perform
    result = BotApi::Rules::Delete.call(user: current_user, rule_id: rule_id)
    return render(text: result.error_message) unless result.success?

    render text: "Rule ##{result.data[:id]} deleted."
  end
end
