class Bots::Signals::PickSpendableAssetsController < ApplicationController
  before_action :authenticate_user!
  before_action :redirect_if_session_expired, only: :create

  include Bots::Searchable

  def new
    @bot = current_user.bots.signal.new(sanitized_bot_config)
    @api_key = @bot.api_key

    if @api_key.correct?
      prepare_step
      nil if render_asset_page(bot: @bot, asset_field: :quote_asset_id)
    else
      redirect_to new_bots_signals_add_api_key_path
    end
  end

  def create
    if bot_params[:quote_asset_id].present?
      bot = current_user.bots.signal.new(sanitized_bot_config)
      session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)
      redirect_to new_bots_signals_confirm_settings_path
    else
      prepare_step
      render :new, status: :unprocessable_entity
    end
  end

  private

  # View state the :new template needs — shared by `new` and `create`'s 422 re-render.
  def prepare_step
    @bot = current_user.bots.signal.new(sanitized_bot_config)
    @bot.quote_asset_id = nil
    @assets = asset_search_results(@bot, search_params[:query], :quote_asset)
  end

  def redirect_if_session_expired
    render turbo_stream: turbo_stream_redirect(root_path) if session[:bot_config].blank?
  end

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_signal).permit(:quote_asset_id)
  end
end
