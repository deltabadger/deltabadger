# Dual counterpart of the order switch (see the single controller). :currencies
# (base0) is always picked on the SINGLE picker route, so asset-first lands there.
class Bots::DcaDualAssets::OrdersController < ApplicationController
  before_action :authenticate_user!

  include Bots::Wizard::Navigable

  def create
    session[:bot_config] ||= {}
    session[:bot_config]['flow'] = target_flow
    reset_downstream!
    redirect_to step_path(current_order.first)
  end

  private

  def target_flow
    params[:flow] == 'exchange_first' ? 'exchange_first' : 'asset_first'
  end

  def current_step = current_order.first
  def bot_relation = current_user.bots.dca_dual_asset

  def step_path(key)
    case key
    when :currencies  then new_bots_dca_single_assets_pick_buyable_asset_path
    when :currencies2 then new_bots_dca_dual_assets_pick_second_buyable_asset_path
    when :exchange    then new_bots_dca_dual_assets_pick_exchange_path
    when :api         then new_bots_dca_dual_assets_add_api_key_path
    when :spendable   then new_bots_dca_dual_assets_pick_spendable_asset_path
    end
  end
end
