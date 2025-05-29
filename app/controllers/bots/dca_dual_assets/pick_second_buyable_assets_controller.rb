class Bots::DcaDualAssets::PickSecondBuyableAssetsController < ApplicationController
  before_action :authenticate_user!

  def new
    bot_config = session[:bot_config].deep_symbolize_keys

    if bot_config[:settings][:base0_asset_id].blank?
      redirect_to new_bots_dca_dual_assets_pick_first_buyable_asset_path
    else
      @bot = current_user.bots.dca_dual_asset.new(bot_config)
      @bot.base1_asset_id = nil
      @assets = search_results(@bot, search_params[:query], :base_asset)
    end
  end

  def create
    if bot_params[:base1_asset_id].present?
      session[:bot_config].deep_merge!({
        settings: { base1_asset_id: bot_params[:base1_asset_id].to_i }
      }.deep_stringify_keys)
      redirect_to new_bots_dca_dual_assets_pick_exchange_path
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_dca_dual_asset).permit(:base1_asset_id)
  end

  def search_results(bot, query, asset_type)
    available_assets = bot.available_assets_for_current_settings(asset_type: asset_type)
    filtered_assets = filter_assets_by_query(assets: available_assets, query: query)
                      .pluck(:id, :symbol, :name)
    exchanges_data = Exchange.all.pluck(:id, :name_id, :name).each_with_object([]) do |(id, name_id, name), list|
      assets = Exchange.find(id).assets.pluck(:id)
      list << [name_id, name, assets] if assets.any?
    end
    filtered_assets.map do |id, symbol, name|
      exchanges = exchanges_data.select { |_, _, assets| assets.include?(id) }
      [id, symbol, name, exchanges.map { |e_name_id, e_name, _| [e_name_id, e_name] }]
    end
  end

  def filter_assets_by_query(assets:, query:)
    return assets.order(:market_cap_rank, :symbol) if query.blank?

    assets
      .map { |asset| [asset, similarities_for_asset(asset, query.downcase)] }
      .select { |_, similarities| similarities.first >= 0.7 }
      .sort_by { |asset, similarities| [similarities.map(&:-@), asset.market_cap_rank || Float::INFINITY] }
      .map(&:first)
  end

  def similarities_for_asset(asset, query)
    [
      asset.symbol.present? ? JaroWinkler.similarity(asset.symbol.downcase.to_s, query) : 0,
      asset.name.present? ? JaroWinkler.similarity(asset.name.downcase.to_s, query) : 0
    ].sort.reverse
  end
end
