class Bots::DcaIndexes::PickSpendableAssetsController < ApplicationController
  before_action :authenticate_user!
  before_action :require_coingecko_configured

  include Bots::Searchable

  def new
    @bot = current_user.bots.dca_index.new(session[:bot_config])
    @api_key = @bot.api_key

    if @api_key.correct?
      @bot.quote_asset_id = nil
      @assets = asset_search_results(@bot, search_params[:query], :quote_asset)
    else
      redirect_to new_bots_dca_indexes_add_api_key_path
    end
  end

  def create
    if bot_params[:quote_asset_id].present?
      bot = current_user.bots.dca_index.new(session[:bot_config])
      session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)
      redirect_to new_bots_dca_indexes_confirm_settings_path
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  def require_coingecko_configured
    return if AppConfig.coingecko_configured?

    redirect_to new_bots_dca_indexes_setup_coingecko_path
  end

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_dca_index).permit(:quote_asset_id)
  end
end
