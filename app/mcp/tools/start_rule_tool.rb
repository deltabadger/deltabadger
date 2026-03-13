# frozen_string_literal: true

class StartRuleTool < ApplicationMCPTool
  tool_name 'start_rule'
  description 'Start a stopped rule (e.g., withdrawal rule)'

  property :rule_id, type: 'number', required: true, description: 'The rule ID'

  def perform
    user = current_user
    rule = user.rules.find_by(id: rule_id.to_i)

    unless rule
      render text: 'Rule not found.'
      return
    end

    if rule.working?
      render text: "Rule ##{rule.id} is already active (#{rule.status})."
      return
    end

    rule.start
    render text: "Rule ##{rule.id} started."
  end
end
