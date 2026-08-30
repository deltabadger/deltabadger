class Rules::Withdrawals::PickAssetsController < ApplicationController
  before_action :authenticate_user!

  include Bots::Searchable

  def new
    @rule_config = session[:withdrawal_rule_config] || {}
    @exchange = Exchange.find_by(id: @rule_config['exchange_id'])

    if @exchange.blank?
      redirect_to new_rules_withdrawals_pick_exchange_path
    else
      # Build a temporary bot to reuse Searchable concern for asset search — carrying the
      # exchange so the list is scoped to what this exchange actually trades.
      @bot = current_user.bots.dca_single_asset.new(exchange: @exchange)
      @assets = asset_search_results(@bot, search_params[:query], :base_asset)
      nil if render_asset_page(bot: @bot, asset_field: :asset_id)
    end
  end

  def create
    asset_id = params.dig(:bots_dca_single_asset, :asset_id)
    if asset_id.present?
      session[:withdrawal_rule_config] ||= {}
      session[:withdrawal_rule_config]['asset_id'] = asset_id
      # A destination is listed per asset, so changing the asset invalidates whatever was
      # picked for the previous one.
      session[:withdrawal_rule_config].except!('address', 'address_name', 'address_tag', 'network')
      redirect_to new_rules_withdrawals_add_api_key_path
    else
      redirect_to new_rules_withdrawals_pick_asset_path
    end
  end

  private

  def search_params
    params.permit(:query)
  end
end
