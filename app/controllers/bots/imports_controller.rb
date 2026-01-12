require 'csv'

class Bots::ImportsController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  def create
    unless params[:file].present?
      flash[:alert] = t('bot.details.stats.import_no_file')
      redirect_to bot_path(@bot) and return
    end

    result = @bot.import_orders_csv(params[:file])

    if result[:success]
      if result[:imported_count] > 0
        flash[:notice] = t('bot.details.stats.import_success', count: result[:imported_count])
      else
        flash[:notice] = t('bot.details.stats.import_no_new_orders')
      end
    else
      flash[:alert] = result[:error]
    end

    redirect_to bot_path(@bot)
  end
end
