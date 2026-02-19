class Rules::Withdrawals::ConfirmSettingsController < ApplicationController
  before_action :authenticate_user!

  def new
    @rule_config = session[:withdrawal_rule_config] || {}
    @asset = Asset.find_by(id: @rule_config['asset_id'])
    @exchange = Exchange.find_by(id: @rule_config['exchange_id'])

    @address = @rule_config['address']

    if @asset.blank?
      redirect_to new_rules_withdrawals_pick_asset_path
    elsif @exchange.blank?
      redirect_to new_rules_withdrawals_pick_exchange_path
    elsif @address.blank?
      redirect_to new_rules_withdrawals_add_address_path
    else
      @exchange.fetch_withdrawal_fees! unless @exchange.withdrawal_fee_fresh?(asset: @asset)

      @rule = Rules::Withdrawal.new(
        asset: @asset,
        exchange: @exchange,
        address: @address,
        address_tag: @rule_config['address_tag'],
        network: @rule_config['network'],
        max_fee_percentage: @rule_config['max_fee_percentage']
      )

      @chains = @rule.available_chains
    end
  end

  def create
    config = session[:withdrawal_rule_config] || {}
    config.merge!(rule_params.to_h)
    session[:withdrawal_rule_config] = config

    @asset = Asset.find_by(id: config['asset_id'])
    @exchange = Exchange.find_by(id: config['exchange_id'])
    @rule = current_user.rules.build(
      type: 'Rules::Withdrawal',
      asset: @asset,
      exchange: @exchange,
      address: config['address'],
      address_tag: config['address_tag'],
      network: config['network'],
      max_fee_percentage: config['max_fee_percentage']
    )

    if @rule.save
      session[:withdrawal_rule_config] = nil
      render turbo_stream: turbo_stream_redirect(rules_path)
    else
      flash.now[:alert] = @rule.errors.full_messages.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  private

  def rule_params
    params.permit(:max_fee_percentage, :network)
  end
end
