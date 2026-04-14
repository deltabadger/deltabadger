class Bots::DcaSingleAssets::PickSpendableAssetsController < ApplicationController
  before_action :authenticate_user!
  before_action :redirect_if_session_expired, only: :create

  include Bots::Searchable

  def new
    @bot = current_user.bots.dca_single_asset.new(sanitized_bot_config)
    @api_key = @bot.api_key

    unless @api_key.correct?
      redirect_to new_bots_dca_single_assets_add_api_key_path
      return
    end

    @bot.quote_asset_id = nil
    @assets = asset_search_results(@bot, search_params[:query], :quote_asset)
    nil if render_asset_page(bot: @bot, asset_field: :quote_asset_id)
  end

  def create
    if bot_params[:quote_asset_id].present?
      bot = current_user.bots.dca_single_asset.new(sanitized_bot_config)
      session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)
      finalise_and_redirect
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  def finalise_and_redirect
    # Defaults for a freshly-created (unstarted) bot; the user edits on the show page.
    session[:bot_config]['settings'] ||= {}
    session[:bot_config]['settings']['quote_amount'] ||= 100
    session[:bot_config]['settings']['interval']     ||= 'week'
    @bot = current_user.bots.dca_single_asset.new(sanitized_bot_config.deep_symbolize_keys)
    @bot.set_missed_quote_amount
    if @bot.save
      session[:bot_config] = nil
      # Break out of the modal_content Turbo frame — go to the full bot show page.
      render turbo_stream: turbo_stream_redirect(bot_path(@bot))
    else
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render :new, status: :unprocessable_entity
    end
  end

  def redirect_if_session_expired
    render turbo_stream: turbo_stream_redirect(root_path) if session[:bot_config].blank?
  end

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_dca_single_asset).permit(:quote_asset_id)
  end
end
