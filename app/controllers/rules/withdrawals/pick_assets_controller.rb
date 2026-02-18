class Rules::Withdrawals::PickAssetsController < ApplicationController
  before_action :authenticate_user!

  include Bots::Searchable

  def new
    session[:withdrawal_rule_config] ||= {}
    # Build a temporary bot to reuse Searchable concern for asset search
    @bot = current_user.bots.dca_single_asset.new
    @assets = asset_search_results(@bot, search_params[:query], :base_asset)
    nil if render_asset_page(bot: @bot, asset_field: :asset_id)
  end

  def create
    asset_id = params.dig(:bots_dca_single_asset, :asset_id)
    if asset_id.present?
      session[:withdrawal_rule_config] ||= {}
      session[:withdrawal_rule_config]['asset_id'] = asset_id
      session[:withdrawal_rule_config].delete('exchange_id')
      redirect_to new_rules_withdrawals_pick_exchange_path
    else
      redirect_to new_rules_withdrawals_pick_asset_path
    end
  end

  private

  def search_params
    params.permit(:query)
  end
end
