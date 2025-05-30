class Bots::DcaDualAssets::ConfirmSettingsController < ApplicationController
  before_action :authenticate_user!

  def new
    session[:bot_config]['settings']['interval'] ||= 'day'
    session[:bot_config]['settings']['allocation0'] ||= 0.5
    bot_config = session[:bot_config].deep_symbolize_keys
    @bot = current_user.bots.dca_dual_asset.new(bot_config)
  end

  def create # rubocop:disable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity
    session[:bot_config].deep_merge!({
      settings: {
        quote_amount: bot_params[:quote_amount].presence&.to_f,
        interval: bot_params[:interval].presence,
        allocation0: bot_params[:allocation0].presence&.to_f,
        marketcap_allocated: bot_params[:marketcap_allocated].presence&.in?(%w[1 true]),
        quote_amount_limited: bot_params[:quote_amount_limited].presence&.in?(%w[1 true]),
        quote_amount_limit: bot_params[:quote_amount_limit].presence&.to_f,
        price_limited: bot_params[:price_limited].presence&.in?(%w[1 true]),
        price_limit: bot_params[:price_limit].presence&.to_f,
        price_limit_timing_condition: bot_params[:price_limit_timing_condition].presence,
        price_limit_price_condition: bot_params[:price_limit_price_condition].presence,
        price_limit_in_ticker_id: bot_params[:price_limit_in_ticker_id].presence&.to_i,
        smart_intervaled: bot_params[:smart_intervaled].presence&.in?(%w[1 true]),
        smart_interval_quote_amount: bot_params[:smart_interval_quote_amount].presence&.to_f
      }.compact
    }.deep_stringify_keys)
    bot_config = session[:bot_config].deep_symbolize_keys
    @bot = current_user.bots.dca_dual_asset.new(bot_config)
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
