class Bots::DcaIndexes::PickIndicesController < ApplicationController
  before_action :authenticate_user!
  before_action :require_market_data_configured

  def new
    session[:bot_config] ||= {}
    # Load internal indices first (Top Coins), then weighted/popular categories, then by market cap
    @indices = Index.order(
      Arel.sql("CASE WHEN source = '#{Index::SOURCE_INTERNAL}' THEN 0 ELSE 1 END"),
      weight: :desc,
      market_cap: :desc
    )
    @assets_by_coingecko_id = fetch_assets_for_indices(@indices)
  end

  def create
    session[:bot_config] ||= {}
    session[:bot_config]['settings'] ||= {}

    index_type = params[:index_type]
    index_category_id = params[:index_category_id]
    index_name = params[:index_name]

    if index_type == Bots::DcaIndex::INDEX_TYPE_TOP
      session[:bot_config]['settings'].merge!({
                                                'index_type' => Bots::DcaIndex::INDEX_TYPE_TOP,
                                                'index_category_id' => nil,
                                                'index_name' => nil
                                              })
      redirect_to new_bots_dca_indexes_pick_exchange_path
    elsif index_type == Bots::DcaIndex::INDEX_TYPE_CATEGORY && index_category_id.present?
      session[:bot_config]['settings'].merge!({
                                                'index_type' => Bots::DcaIndex::INDEX_TYPE_CATEGORY,
                                                'index_category_id' => index_category_id,
                                                'index_name' => index_name
                                              })
      redirect_to new_bots_dca_indexes_pick_exchange_path
    else
      flash.now[:alert] = t('bot.dca_index.setup.pick_index.error')
      @indices = Index.order(
        Arel.sql("CASE WHEN source = '#{Index::SOURCE_INTERNAL}' THEN 0 ELSE 1 END"),
        weight: :desc,
        market_cap: :desc
      )
      @assets_by_coingecko_id = fetch_assets_for_indices(@indices)
      render :new, status: :unprocessable_entity
    end
  end

  private

  def require_market_data_configured
    return if MarketData.configured?

    redirect_to new_bots_dca_indexes_setup_coingecko_path
  end

  def fetch_assets_for_indices(indices)
    coingecko_ids = indices.flat_map { |i| i.top_coins || [] }.uniq
    return {} if coingecko_ids.empty?

    Asset.where(external_id: coingecko_ids).index_by(&:external_id)
  end
end
