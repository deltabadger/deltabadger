class Settings::PlatformConnectionsController < ApplicationController
  include AdminOnly

  before_action :authenticate_user!
  before_action :require_admin!

  def create
    result = Platform::RedeemClaim.call(code: params[:claim_code])

    if result.success?
      flash.now[:notice] = t('settings.platform.connected')
      render_market_data_widget
    else
      @platform_errors = result.errors
      @show_platform_form = true
      render_market_data_widget(status: :unprocessable_entity)
    end
  end

  def destroy
    ApplicationRecord.transaction do
      AppConfig.clear_market_data_settings!
      proxy_keys = AppConfig.pluck(:key).select { |key| key.start_with?('proxy_') }
      AppConfig.where(key: proxy_keys).delete_all
      AppConfig.delete('platform_connected_at')
    end

    flash.now[:notice] = t('settings.platform.disconnected')
    render_market_data_widget
  end

  private

  def render_market_data_widget(status: :ok)
    streams = [turbo_stream.replace('market_data_settings', partial: 'settings/widgets/market_data')]
    streams << turbo_stream.prepend('flash', partial: 'layouts/flash') if flash.any?
    render turbo_stream: streams, status: status
  end
end
