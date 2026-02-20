class RulesController < ApplicationController
  before_action :authenticate_user!

  def index
    @withdrawal_rules = current_user.rules.where(type: 'Rules::Withdrawal').includes(:exchange, :asset)
    @rule_logs = RuleLog.where(rule_id: current_user.rules.select(:id)).order(created_at: :desc).limit(50)
    @exchanges = Exchange.where(available: true).order(:name)
    @assets = Asset.where(category: 'Cryptocurrency').order(:name)
  end
end
