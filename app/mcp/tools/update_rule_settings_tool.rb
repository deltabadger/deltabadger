# frozen_string_literal: true

class UpdateRuleSettingsTool < ApplicationMCPTool
  tool_name 'update_rule_settings'
  description 'Update settings on a stopped rule (max_fee_percentage, min_amount, threshold_type)'

  property :rule_id, type: 'number', required: true, description: 'The rule ID'
  property :max_fee_percentage, type: 'number', description: 'Max acceptable withdrawal fee percentage (optional)'
  property :min_amount, type: 'number', description: 'Minimum amount threshold (optional)'
  property :threshold_type, type: 'string',
                            description: "'fee_percentage' or 'min_amount' — which threshold to use (optional)"

  def perform
    user = current_user
    rule = user.rules.find_by(id: rule_id.to_i)

    unless rule
      render text: 'Rule not found.'
      return
    end

    if rule.working?
      render text: "Rule must be stopped before updating settings. Current status: #{rule.status}."
      return
    end

    updates = {}
    updates[:max_fee_percentage] = max_fee_percentage.to_s if max_fee_percentage.present?
    updates[:min_amount] = min_amount.to_s if min_amount.present?
    updates[:threshold_type] = threshold_type if threshold_type.present?

    if updates.empty?
      render text: 'No settings provided to update.'
      return
    end

    rule.max_fee_percentage = updates[:max_fee_percentage] if updates[:max_fee_percentage]
    rule.min_amount = updates[:min_amount] if updates[:min_amount]
    rule.threshold_type = updates[:threshold_type] if updates[:threshold_type]

    if rule.save
      render text: "Rule ##{rule.id} settings updated: #{updates.keys.join(', ')}."
    else
      render text: "Failed to update rule: #{rule.errors.full_messages.join(', ')}"
    end
  end
end
