class Bots::DcaDualAssets::ConfirmSettingsController < ApplicationController
  before_action :authenticate_user!

  def new
    session[:bot_config]['settings']['interval'] ||= 'day'
    session[:bot_config]['settings']['allocation0'] ||= 0.5
    @bot = current_user.bots.dca_dual_asset.new(session[:bot_config])
  end

  def create
    bot = current_user.bots.dca_dual_asset.new(session[:bot_config])
    session[:bot_config].deep_merge!({ settings: bot.parsed_settings(bot_params) }.deep_stringify_keys)
    @bot = current_user.bots.dca_dual_asset.new(session[:bot_config])
    return render :create if @bot.quote_amount.blank? || @bot.valid?

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render :create, status: :unprocessable_entity
  end

  private

  def bot_params
    params.require(:bots_dca_dual_asset).permit(
      :quote_amount,
      :interval,
      :allocation0,
      :marketcap_allocated,
      :quote_amount_limited,
      :quote_amount_limit,
      :price_limited,
      :price_limit,
      :price_limit_timing_condition,
      :price_limit_price_condition,
      :price_limit_in_ticker_id,
      :smart_intervaled,
      :smart_interval_quote_amount
    )
  end
end
