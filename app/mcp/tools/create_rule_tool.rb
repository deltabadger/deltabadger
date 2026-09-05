# frozen_string_literal: true

class CreateRuleTool < ApplicationMCPTool
  tool_name 'create_rule'
  description "Create a withdrawal rule (stopped). The address must already be on the exchange's withdrawal allow-list. Use start_rule to activate."

  property :exchange_name, type: 'string', required: true, description: 'Exchange name (e.g., Binance)'
  property :asset, type: 'string', required: true, description: 'Asset symbol to withdraw (e.g., BTC)'
  property :address, type: 'string', required: true, description: 'Destination address, exactly as allow-listed on the exchange'
  property :address_tag, type: 'string', description: 'Memo / tag, if the network needs one'
  property :network, type: 'string', description: 'Network, if the exchange needs one'
  property :withdrawal_percentage, type: 'number', description: 'Share of the free balance to withdraw (default 100)'
  property :threshold_type, type: 'string', description: "'fee_percentage' (default) or 'min_amount'"
  property :max_fee_percentage, type: 'number', description: 'Withdraw only when the fee is at most this percent (default 0.5)'
  property :min_amount, type: 'number', description: "Withdraw only above this amount (for threshold_type 'min_amount')"

  def perform
    result = BotApi::Rules::Create.call(
      user: current_user, exchange_name: exchange_name, asset: asset, address: address,
      address_tag: address_tag, network: network, withdrawal_percentage: withdrawal_percentage,
      threshold_type: threshold_type, max_fee_percentage: max_fee_percentage, min_amount: min_amount
    )
    return render(text: result.error_message) unless result.success?

    d = result.data
    render text: "Rule ##{d[:id]} created (stopped): #{d[:withdrawal_percentage]}% of #{d[:asset]} on " \
                 "#{d[:exchange]} to #{d[:address]}. Use 'start_rule' to activate."
  end
end
