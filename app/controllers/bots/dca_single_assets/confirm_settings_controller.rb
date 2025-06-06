class Bots::DcaSingleAssets::ConfirmSettingsController < ApplicationController
  before_action :authenticate_user!

  def new
    session[:bot_config]['settings']['interval'] ||= 'day'
    @bot = current_user.bots.dca_single_asset.new(session[:bot_config])
  end

  def create
    bot = current_user.bots.dca_single_asset.new(session[:bot_config])
    session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)
    @bot = current_user.bots.dca_single_asset.new(session[:bot_config])
    return render :create if @bot.quote_amount.blank? || @bot.valid?

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render :create, status: :unprocessable_entity
  end

  private

  def bot_params
    params.require(:bots_dca_single_asset).permit(*permitted_settings)
  end

  def permitted_settings
    Bots::DcaSingleAsset.stored_attributes[:settings].reject do |key|
      key.in?(%i[
                base_asset_id
                quote_asset_id
              ])
    end
  end
end
