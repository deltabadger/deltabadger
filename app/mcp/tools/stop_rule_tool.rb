# frozen_string_literal: true

class StopRuleTool < ApplicationMCPTool
  tool_name 'stop_rule'
  description 'Stop an active rule'

  property :rule_id, type: 'number', required: true, description: 'The rule ID'

  def perform
    user = current_user
    rule = user.rules.find_by(id: rule_id.to_i)

    unless rule
      render text: 'Rule not found.'
      return
    end

    unless rule.working?
      render text: "Rule ##{rule.id} is not active (#{rule.status})."
      return
    end

    rule.stop
    render text: "Rule ##{rule.id} stopped."
  end
end
