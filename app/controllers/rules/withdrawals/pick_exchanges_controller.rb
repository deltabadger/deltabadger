class Rules::Withdrawals::PickExchangesController < ApplicationController
  before_action :authenticate_user!

  include Bots::Searchable

  def new
    @rule_config = session[:withdrawal_rule_config] || {}
    @asset = Asset.find_by(id: @rule_config['asset_id'])

    if @asset.blank?
      redirect_to new_rules_withdrawals_pick_asset_path
    else
      # Build a temporary bot to reuse Searchable concern for exchange search
      @bot = current_user.bots.dca_single_asset.new(base_asset_id: @asset.id)
      @exchanges = exchange_search_results(@bot, search_params[:query])
    end
  end

  def create
    exchange_id = params.dig(:bots_dca_single_asset, :exchange_id)
    if exchange_id.present?
      session[:withdrawal_rule_config]['exchange_id'] = exchange_id
      redirect_to new_rules_withdrawals_add_api_key_path
    else
      redirect_to new_rules_withdrawals_pick_exchange_path
    end
  end

  private

  def search_params
    params.permit(:query)
  end
end
