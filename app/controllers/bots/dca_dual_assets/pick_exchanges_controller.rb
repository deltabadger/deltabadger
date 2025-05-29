class Bots::DcaDualAssets::PickExchangesController < ApplicationController
  before_action :authenticate_user!

  def new
    bot_config = session[:bot_config].deep_symbolize_keys

    if bot_config[:settings][:base1_asset_id].blank?
      redirect_to new_bots_dca_dual_assets_pick_second_buyable_asset_path
    else
      @bot = current_user.bots.dca_dual_asset.new(bot_config)
      @bot.exchange_id = nil
      @exchanges = search_results(@bot, search_params[:query])
    end
  end

  def create
    if bot_params[:exchange_id].present?
      session[:bot_config].deep_merge!({
        exchange_id: bot_params[:exchange_id]
      }.deep_stringify_keys)
      redirect_to new_bots_dca_dual_assets_add_api_key_path
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_dca_dual_asset).permit(:exchange_id)
  end

  def search_results(bot, query)
    exchanges = bot.available_exchanges_for_current_settings
    filter_exchanges_by_query(exchanges: exchanges, query: query)
  end

  def filter_exchanges_by_query(exchanges:, query:)
    return exchanges.order(:name) if query.blank?

    exchanges
      .map { |exchange| [exchange, similarities_for_exchange(exchange, query.downcase)] }
      .select { |_, similarities| similarities.first >= 0.7 }
      .sort_by { |_, similarities| similarities.map(&:-@) }
      .map(&:first)
  end

  def similarities_for_exchange(exchange, query)
    [
      exchange.name.present? ? JaroWinkler.similarity(exchange.name.downcase.to_s, query) : 0
    ].sort.reverse
  end
end
