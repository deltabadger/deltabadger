class Rules::Withdrawals::AddAddressesController < ApplicationController
  before_action :authenticate_user!

  def new
    @rule_config = session[:withdrawal_rule_config] || {}
    @asset = Asset.find_by(id: @rule_config['asset_id'])
    @exchange = Exchange.find_by(id: @rule_config['exchange_id'])

    if @asset.blank?
      redirect_to new_rules_withdrawals_pick_asset_path
    elsif @exchange.blank?
      redirect_to new_rules_withdrawals_pick_exchange_path
    else
      @address = @rule_config['address']
      @address_tag = @rule_config['address_tag']
    end
  end

  def create
    session[:withdrawal_rule_config] ||= {}
    session[:withdrawal_rule_config]['address'] = params[:address]
    session[:withdrawal_rule_config]['address_tag'] = params[:address_tag].presence
    redirect_to new_rules_withdrawals_confirm_settings_path
  end
end
