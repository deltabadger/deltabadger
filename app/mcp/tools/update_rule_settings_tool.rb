# frozen_string_literal: true

class UpdateRuleSettingsTool < ApplicationMCPTool
  tool_name 'update_rule_settings'
  description 'Update settings on a stopped rule (withdrawal_percentage, max_fee_percentage, min_amount, threshold_type)'

  property :rule_id, type: 'number', required: true, description: 'The rule ID'
  property :withdrawal_percentage, type: 'number', description: 'Percentage of available balance to withdraw (optional)'
  property :max_fee_percentage, type: 'number', description: 'Max acceptable withdrawal fee percentage (optional)'
  property :min_amount, type: 'number', description: 'Minimum amount threshold (optional)'
  property :threshold_type, type: 'string',
                            description: "'fee_percentage' or 'min_amount' — which threshold to use (optional)"

  def perform
    result = BotApi::Rules::UpdateSettings.call(
      user: current_user, rule_id: rule_id,
      withdrawal_percentage: withdrawal_percentage,
      max_fee_percentage: max_fee_percentage,
      min_amount: min_amount,
      threshold_type: threshold_type
    )
    return render(text: result.error_message) unless result.success?

    render text: "Rule ##{result.data[:id]} settings updated: #{result.data[:updated].join(', ')}."
  end
end
