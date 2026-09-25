class Bots::DcaIndexes::PickIndicesController < ApplicationController
  before_action :authenticate_user!
  before_action :require_market_data_configured

  def new
    session[:bot_config] ||= {}
    @indices = Index.for_picker
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
                                                'index_name' => nil,
                                                'index_name_prefix' => nil
                                              })
      redirect_to new_bots_dca_indexes_pick_exchange_path
    elsif index_type == Bots::DcaIndex::INDEX_TYPE_CATEGORY && index_category_id.present?
      session[:bot_config]['settings'].merge!({
                                                'index_type' => Bots::DcaIndex::INDEX_TYPE_CATEGORY,
                                                'index_category_id' => index_category_id,
                                                'index_name' => index_name,
                                                'index_name_prefix' => Bots::DcaIndex::COUNT_NAMED_INDICES.dig(index_category_id, :prefix)
                                              })
      redirect_to new_bots_dca_indexes_pick_exchange_path
    else
      flash.now[:alert] = t('bot.dca_index.setup.pick_index.error')
      @indices = Index.for_picker
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
