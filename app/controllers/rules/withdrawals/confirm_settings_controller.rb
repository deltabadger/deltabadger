class Rules::Withdrawals::ConfirmSettingsController < ApplicationController
  before_action :authenticate_user!

  def new
    @rule_config = session[:withdrawal_rule_config] || {}
    @asset = Asset.find_by(id: @rule_config['asset_id'])
    @exchange = Exchange.find_by(id: @rule_config['exchange_id'])

    @address = @rule_config['address']

    if @exchange.blank?
      redirect_to new_rules_withdrawals_pick_exchange_path
    elsif @asset.blank?
      redirect_to new_rules_withdrawals_pick_asset_path
    elsif @address.blank?
      redirect_to new_rules_withdrawals_add_address_path
    else
      setup_rule_from_config(@rule_config)
    end
  end

  def preview
    config = session[:withdrawal_rule_config] || {}
    config.merge!(rule_params.to_h)
    session[:withdrawal_rule_config] = config

    @asset = Asset.find_by(id: config['asset_id'])
    @exchange = Exchange.find_by(id: config['exchange_id'])
    @address = config['address']

    setup_rule_from_config(config)

    respond_to do |format|
      format.turbo_stream { render turbo_stream: turbo_stream.replace('rule-preview', partial: 'rule_preview') }
    end
  end

  def create
    config = session[:withdrawal_rule_config] || {}
    config.merge!(rule_params.to_h)
    session[:withdrawal_rule_config] = config

    @asset = Asset.find_by(id: config['asset_id'])
    @exchange = Exchange.find_by(id: config['exchange_id'])
    @address = config['address']

    return redirect_to new_rules_withdrawals_add_address_path unless allowlisted_address?(@address)

    existing = current_user.rules.find_by(type: 'Rules::Withdrawal', asset: @asset, exchange: @exchange)
    if existing && !existing.deleted?
      flash.now[:alert] = t('errors.withdrawal_rule_already_exists',
                            asset_symbol: @asset.symbol, exchange_name: @exchange.name)
      return render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end

    @rule = existing

    if @rule
      @rule.assign_attributes(
        address: config['address'],
        address_name: config['address_name'],
        address_tag: config['address_tag'],
        network: config['network'],
        withdrawal_percentage: config['withdrawal_percentage'],
        threshold_type: config['threshold_type'],
        max_fee_percentage: config['max_fee_percentage'],
        min_amount: config['min_amount'],
        max_interval: config['max_interval'].presence,
        status: :created
      )
    else
      @rule = current_user.rules.build(
        type: 'Rules::Withdrawal',
        asset: @asset,
        exchange: @exchange,
        address: config['address'],
        address_name: config['address_name'],
        address_tag: config['address_tag'],
        network: config['network'],
        withdrawal_percentage: config['withdrawal_percentage'],
        threshold_type: config['threshold_type'],
        max_fee_percentage: config['max_fee_percentage'],
        min_amount: config['min_amount'],
        max_interval: config['max_interval'].presence
      )
    end

    if @rule.save
      session[:withdrawal_rule_config] = nil
      render turbo_stream: turbo_stream_redirect(rules_path)
    else
      flash.now[:alert] = @rule.errors.full_messages.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  private

  def setup_rule_from_config(config)
    api_key = current_user.api_keys.find_by(exchange: @exchange, key_type: :withdrawal)
    if api_key
      @exchange.set_client(api_key: api_key)
      @withdrawal_addresses = @exchange.list_withdrawal_addresses(asset: @asset)

      if @withdrawal_addresses.is_a?(Array) && @withdrawal_addresses.any?
        if @withdrawal_addresses.none? { |a| a[:name] == @address }
          session[:withdrawal_rule_config].delete('address')
          session[:withdrawal_rule_config].delete('address_name')
          redirect_to new_rules_withdrawals_add_address_path
          return
        end

        selected = @withdrawal_addresses.find { |a| a[:name] == @address }
        session[:withdrawal_rule_config]['address_name'] = selected[:key] if selected
      end
    end

    unless @exchange.withdrawal_fee_fresh?(asset: @asset)
      @exchange.set_client(api_key: api_key) if api_key && @exchange.api_key.blank?
      @exchange.fetch_withdrawal_fees!
    end

    @rule = Rules::Withdrawal.new(
      asset: @asset,
      exchange: @exchange,
      address: @address,
      address_tag: config['address_tag'],
      withdrawal_percentage: config['withdrawal_percentage'],
      threshold_type: config['threshold_type'],
      max_fee_percentage: config['max_fee_percentage'],
      min_amount: config['min_amount']
    )

    @chains = @rule.available_chains

    fee_known = @rule.withdrawal_fee_known?
    @rule.threshold_type ||= fee_known ? 'fee_percentage' : 'min_amount'
    @rule.withdrawal_percentage = @rule.withdrawal_percentage.presence || '100'
    @rule.max_fee_percentage ||= '0.5'
    @rule.min_amount ||= '0.1'
    @rule.network = config['network'] || @chains.find { |c| c['is_default'] }&.dig('name') || @chains.first&.dig('name')
  end

  # :address stays permitted: the confirmation screen is where the destination is chosen,
  # from a select built out of the exchange's own allowlist (_rule_preview.html.erb), and
  # dropping it here would pin every rule to whichever address happened to be auto-selected
  # first. What it must not be is trusted because a screen offered it — see
  # allowlisted_address?, which every write path now passes through.
  def rule_params
    params.permit(:withdrawal_percentage, :threshold_type, :max_fee_percentage, :min_amount, :max_interval, :network, :address)
  end

  # The membership test used to run only while preparing this screen, so it bounded what the
  # screen offered rather than what the app accepted, and #create never called it.
  #
  # Fails closed. An exchange that cannot be asked has not told us this address is allowed,
  # and on a path that moves funds that is the safe way to be wrong: a legitimate user
  # reached this screen through a successful listing, so refusing here costs them a retry
  # rather than a wrong destination.
  def allowlisted_address?(address)
    return false if address.blank? || @asset.blank? || @exchange.blank?

    api_key = current_user.api_keys.find_by(exchange: @exchange, key_type: :withdrawal)
    return false unless api_key&.correct?

    @exchange.set_client(api_key: api_key)
    addresses = @exchange.list_withdrawal_addresses(asset: @asset)
    return false if addresses.blank?

    addresses.any? { |candidate| candidate[:name] == address }
  end
end
