# POST-only switch between asset-first and exchange-first ordering. Sets the flow
# variant, wipes whatever was picked so far (re-toggling at step one discards at
# most one pick), and redirects to the target variant's first step. There is no
# GET action on purpose — Turbo prefetches GETs on hover, which must never flip
# the variant.
class Bots::DcaSingleAssets::OrdersController < ApplicationController
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

  # On the order switch the "current step" is the target variant's first step, so
  # reset_downstream! wipes the whole config (keeping label + the new flow).
  def current_step = current_order.first
  def bot_relation = current_user.bots.dca_single_asset

  def step_path(key)
    case key
    when :currencies then new_bots_dca_single_assets_pick_buyable_asset_path
    when :exchange   then new_bots_dca_single_assets_pick_exchange_path
    when :api        then new_bots_dca_single_assets_add_api_key_path
    when :spendable  then new_bots_dca_single_assets_pick_spendable_asset_path
    end
  end
end
