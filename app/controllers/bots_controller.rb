class BotsController < ApplicationController
  before_action :authenticate_user!

  def show
    @bot = current_user.bots.find(params[:id])
    respond_to do |format|
      format.html { render 'home/dashboard' } # Render the dashboard view on refresh
      format.json { render json: @bot }
    end
  rescue ActiveRecord::RecordNotFound
    redirect_to dashboard_path, alert: t('bots.not_found')
  end
end
