# frozen_string_literal: true

class ListRulesTool < ApplicationMCPTool
  tool_name 'list_rules'
  description 'List withdrawal rules with their id, status, exchange, asset, destination and thresholds'
  read_only

  def perform
    data = BotApi::Rules::List.call(user: current_user).data
    return render(text: 'No rules found.') if data[:count].zero?

    lines = data[:rules].map do |r|
      threshold = r[:threshold_type] == 'min_amount' ? "min #{r[:min_amount]}" : "max fee #{r[:max_fee_percentage]}%"
      "- ##{r[:id]} | #{r[:asset]} on #{r[:exchange]} | #{r[:status]} | #{r[:withdrawal_percentage]}% to #{r[:address]} | #{threshold}"
    end
    render text: "Rules (#{data[:count]}):\n#{lines.join("\n")}"
  end
end
