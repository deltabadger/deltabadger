class Bots::AddApiKeysController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  def new
    @api_key = @bot.api_key
  end

  def create
    @api_key = @bot.api_key
    @api_key.key = api_key_params[:key]
    @api_key.secret = api_key_params[:secret]
    @api_key.validate_key_permissions
    if @api_key.correct? && @api_key.save
      flash[:notice] = t('errors.bots.api_key_success')
      render turbo_stream: turbo_stream_page_refresh
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  def api_key_params
    params.require(:api_key).permit(:key, :secret)
  end
end
