class RulesController < ApplicationController
  before_action :authenticate_user!

  def index
    @withdrawal_rules = current_user.rules.where(type: 'Rules::Withdrawal').where.not(status: :deleted).includes(:exchange, :asset)
    @rule_logs = RuleLog.where(rule_id: current_user.rules.select(:id)).order(created_at: :desc).limit(50)
    @exchanges = Exchange.where(available: true).order(:name)
    @assets = Asset.where(category: 'Cryptocurrency').order(:name)

    @withdrawal_addresses = {}
    @withdrawal_rules.select(&:stopped?).each do |rule|
      api_key = current_user.api_keys.find_by(exchange: rule.exchange, key_type: :withdrawal)
      next unless api_key

      rule.exchange.set_client(api_key: api_key)
      @withdrawal_addresses[rule.id] = rule.exchange.list_withdrawal_addresses(asset: rule.asset)
    end
  end
end
