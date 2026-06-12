class Bots::Signals::PickBuyableAssetsController < ApplicationController
  before_action :authenticate_user!

  include Bots::Searchable

  def new
    session[:bot_config] ||= {}
    prepare_step
    session[:bot_config]['label'] ||= @bot.label
    nil if render_asset_page(bot: @bot, asset_field: :base_asset_id)
  end

  def create
    # First wizard step: a POST with an expired session is self-sufficient,
    # mirroring the single-asset first step — re-initialise rather than bail.
    session[:bot_config] ||= {}
    if bot_params[:base_asset_id].present?
      bot = current_user.bots.signal.new(sanitized_bot_config)
      session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)
      redirect_to new_bots_signals_pick_exchange_path
    else
      prepare_step
      render :new, status: :unprocessable_entity
    end
  end

  private

  # View state the :new template needs — shared by `new` and `create`'s 422 re-render.
  def prepare_step
    @bot = current_user.bots.signal.new(sanitized_bot_config)
    @bot.base_asset_id = nil
    @bot.exchange_id = nil
    @assets = asset_search_results(@bot, search_params[:query], :base_asset)
  end

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_signal).permit(:base_asset_id)
  end
end
