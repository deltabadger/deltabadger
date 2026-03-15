class Bots::Signals::PickBuyableAssetsController < ApplicationController
  before_action :authenticate_user!

  include Bots::Searchable

  def new
    session[:bot_config] ||= {}
    @bot = current_user.bots.signal.new(sanitized_bot_config)
    @bot.base_asset_id = nil
    @bot.exchange_id = nil
    session[:bot_config]['label'] ||= @bot.label
    @assets = asset_search_results(@bot, search_params[:query], :base_asset)
    nil if render_asset_page(bot: @bot, asset_field: :base_asset_id)
  end

  def create
    if bot_params[:base_asset_id].present?
      bot = current_user.bots.signal.new(sanitized_bot_config)
      session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)
      redirect_to new_bots_signals_pick_exchange_path
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_signal).permit(:base_asset_id)
  end
end
