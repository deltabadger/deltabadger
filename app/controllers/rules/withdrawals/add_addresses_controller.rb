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
      result = auto_select_withdrawal_address
      case result
      when :selected
        redirect_to new_rules_withdrawals_confirm_settings_path
      when :no_addresses
        @no_addresses = true
      when :no_listing
        @no_listing = true
      when :no_key
        redirect_to new_rules_withdrawals_add_api_key_path
      end
    end
  end

  def create
    session[:withdrawal_rule_config] ||= {}
    session[:withdrawal_rule_config]['address'] = params[:address]
    session[:withdrawal_rule_config]['address_tag'] = params[:address_tag].presence
    redirect_to new_rules_withdrawals_confirm_settings_path
  end

  private

  def auto_select_withdrawal_address
    api_key = current_user.api_keys.find_by(exchange: @exchange, key_type: :withdrawal)
    return :no_key unless api_key&.correct?

    @exchange.set_client(api_key: api_key)
    addresses = @exchange.list_withdrawal_addresses(asset: @asset)
    return :no_listing if addresses.nil?
    return :no_addresses if addresses.empty?

    session[:withdrawal_rule_config] ||= {}
    session[:withdrawal_rule_config]['address'] ||= addresses.first[:name]
    session[:withdrawal_rule_config]['address_name'] ||= addresses.first[:key]
    :selected
  end
end
