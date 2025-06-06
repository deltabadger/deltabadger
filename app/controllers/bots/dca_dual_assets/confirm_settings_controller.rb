class Bots::DcaDualAssets::ConfirmSettingsController < ApplicationController
  before_action :authenticate_user!

  def new
    session[:bot_config]['settings']['interval'] ||= 'day'
    session[:bot_config]['settings']['allocation0'] ||= 0.5
    @bot = current_user.bots.dca_dual_asset.new(session[:bot_config])
  end

  def create
    bot = current_user.bots.dca_dual_asset.new(session[:bot_config])
    session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)
    @bot = current_user.bots.dca_dual_asset.new(session[:bot_config])
    return render :create if @bot.quote_amount.blank? || @bot.valid?

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render :create, status: :unprocessable_entity
  end

  private

  def bot_params
    params.require(:bots_dca_dual_asset).permit(*permitted_settings)
  end

  def permitted_settings
    Bots::DcaDualAsset.stored_attributes[:settings].reject do |key|
      key.in?(%i[
                base0_asset_id
                base1_asset_id
                quote_asset_id
              ])
    end
  end
end
