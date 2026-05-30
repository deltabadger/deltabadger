class Bots::DcaIndexes::PickSpendableAssetsController < ApplicationController
  before_action :authenticate_user!
  before_action :require_market_data_configured

  include Bots::Searchable

  def new
    @bot = current_user.bots.dca_index.new(sanitized_bot_config)
    @api_key = @bot.api_key

    if @api_key.correct?
      @bot.quote_asset_id = nil
      @assets = asset_search_results(@bot, search_params[:query], :quote_asset)
      # Per-currency preview of the chosen index's top base assets, keyed by
      # quote_asset_id. The view renders this as a `.ticker-group` cell in
      # place of the generic exchange-icons cell — same affordance as the
      # ticker pills shown on the index tiles in step 2.
      @top_base_assets_by_quote = @bot.top_base_assets_by_quote_for_current_setup
    else
      redirect_to new_bots_dca_indexes_add_api_key_path
    end
  end

  def create
    if bot_params[:quote_asset_id].present?
      bot = current_user.bots.dca_index.new(sanitized_bot_config)
      session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)
      finalise_and_redirect
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  # Mirrors `Bots::DcaSingleAssets::PickSpendableAssetsController#finalise_and_redirect`:
  # create the index bot in its initial (`:created`) state — not started —
  # then break out of the `modal_content` Turbo frame to the bot's show page
  # where the user fine-tunes settings. Defaults match what
  # `Bots::DcaIndexes::ConfirmSettingsController#new` previously applied so
  # the resulting bot has the same shape as before.
  def finalise_and_redirect
    session[:bot_config]['settings'] ||= {}
    session[:bot_config]['settings']['quote_amount']         ||= 100
    session[:bot_config]['settings']['interval']             ||= 'week'
    # num_coins default is owned by the model (index-aware: a bounded index starts at full size).
    session[:bot_config]['settings']['allocation_flattening'] ||= 0.0
    @bot = current_user.bots.dca_index.new(sanitized_bot_config.deep_symbolize_keys)
    @bot.set_missed_quote_amount
    if @bot.save
      session[:bot_config] = nil
      render turbo_stream: turbo_stream_redirect(bot_path(@bot))
    else
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render :new, status: :unprocessable_entity
    end
  end

  def require_market_data_configured
    return if MarketData.configured?

    redirect_to new_bots_dca_indexes_setup_coingecko_path
  end

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_dca_index).permit(:quote_asset_id)
  end
end
