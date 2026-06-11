class Bots::Signals::PickExchangesController < ApplicationController
  before_action :authenticate_user!

  include Bots::Searchable

  def new
    @bot = current_user.bots.signal.new(sanitized_bot_config)

    if @bot.base_asset_id.blank?
      redirect_to new_bots_signals_pick_buyable_asset_path
    else
      prepare_step
    end
  end

  def create
    if bot_params[:exchange_id].present?
      session[:bot_config].merge!({ exchange_id: bot_params[:exchange_id] }.stringify_keys)
      redirect_to new_bots_signals_add_api_key_path
    else
      prepare_step
      render :new, status: :unprocessable_entity
    end
  end

  private

  # View state the :new template needs — shared by `new` and `create`'s 422 re-render.
  def prepare_step
    @bot = current_user.bots.signal.new(sanitized_bot_config)
    @bot.exchange_id = nil
    @exchanges = exchange_search_results(@bot, search_params[:query])
  end

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_signal).permit(:exchange_id)
  end
end
