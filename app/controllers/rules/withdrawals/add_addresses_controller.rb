class Rules::Withdrawals::AddAddressesController < ApplicationController
  before_action :authenticate_user!

  def new
    @rule_config = session[:withdrawal_rule_config] || {}
    @asset = Asset.find_by(id: @rule_config['asset_id'])
    @exchange = Exchange.find_by(id: @rule_config['exchange_id'])

    if @exchange.blank?
      redirect_to new_rules_withdrawals_pick_exchange_path
    elsif @asset.blank?
      redirect_to new_rules_withdrawals_pick_asset_path
    else
      result = auto_select_withdrawal_address
      case result
      when :selected
        redirect_to new_rules_withdrawals_confirm_settings_path
      when :no_addresses
        @no_addresses = true
      when :no_listing
        @no_listing = true
        @api_key = current_user.api_keys.find_or_initialize_by(exchange: @exchange, key_type: :withdrawal)
      when :no_key
        redirect_to new_rules_withdrawals_add_api_key_path
      end
    end
  end

  # This is the only step where an address arrives from outside, so it is the step that has
  # to check it. Until now the membership test lived in the confirmation screen's view-prep,
  # which bounded what that screen offered rather than what the app accepted.
  def create
    @rule_config = session[:withdrawal_rule_config] || {}
    @asset = Asset.find_by(id: @rule_config['asset_id'])
    @exchange = Exchange.find_by(id: @rule_config['exchange_id'])

    return redirect_to new_rules_withdrawals_add_address_path unless allowlisted_address?(params[:address])

    session[:withdrawal_rule_config] ||= {}
    session[:withdrawal_rule_config]['address'] = params[:address]
    session[:withdrawal_rule_config]['address_tag'] = params[:address_tag].presence
    redirect_to new_rules_withdrawals_confirm_settings_path
  end

  private

  # Fails closed. An exchange that cannot be reached, has no key, or returns nothing is not
  # an exchange that has told us this address is allowed — and the whole point of the
  # allowlist is that the destination was authorised somewhere we do not control.
  def allowlisted_address?(address)
    return false if address.blank? || @asset.blank? || @exchange.blank?

    api_key = current_user.api_keys.find_by(exchange: @exchange, key_type: :withdrawal)
    return false unless api_key&.correct?

    @exchange.set_client(api_key: api_key)
    addresses = @exchange.list_withdrawal_addresses(asset: @asset)
    return false if addresses.blank?

    addresses.any? { |candidate| candidate[:name] == address }
  end

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
