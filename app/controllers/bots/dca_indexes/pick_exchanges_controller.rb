class Bots::DcaIndexes::PickExchangesController < ApplicationController
  before_action :authenticate_user!
  before_action :require_coingecko_configured
  before_action :require_index_selected

  include Bots::Searchable

  def new
    session[:bot_config] ||= {}
    @bot = current_user.bots.dca_index.new(session[:bot_config])
    @bot.exchange_id = nil
    session[:bot_config]['label'] ||= @bot.label
    @exchanges = exchange_search_results_for_index_bot(@bot, search_params[:query])
    load_exchange_coins_data
  end

  def create
    if bot_params[:exchange_id].present?
      session[:bot_config].merge!({ exchange_id: bot_params[:exchange_id] }.stringify_keys)
      redirect_to new_bots_dca_indexes_add_api_key_path
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  def require_coingecko_configured
    return if AppConfig.coingecko_configured?

    redirect_to new_bots_dca_indexes_setup_coingecko_path
  end

  def require_index_selected
    return if session.dig(:bot_config, 'settings', 'index_type').present?

    redirect_to new_bots_dca_indexes_pick_index_path
  end

  def search_params
    params.permit(:query)
  end

  def bot_params
    params.require(:bots_dca_index).permit(:exchange_id)
  end

  def load_exchange_coins_data
    return unless @index.present?

    # Load assets for display
    @assets_by_external_id = Asset.where(external_id: @index.top_coins).index_by(&:external_id)

    # For each exchange, find which top coins have available tickers
    @exchange_top_coins = {}
    @exchanges.each do |exchange|
      available_asset_ids = exchange.tickers.available.pluck(:base_asset_id).to_set
      coins = @index.top_coins.map { |ext_id| @assets_by_external_id[ext_id] }.compact
      @exchange_top_coins[exchange.id] = coins.select { |asset| available_asset_ids.include?(asset.id) }
    end
  end

  def exchange_search_results_for_index_bot(bot, query)
    index_type = session.dig(:bot_config, 'settings', 'index_type')
    index_category_id = session.dig(:bot_config, 'settings', 'index_category_id')

    @index = nil # Store for view to access coin counts

    exchanges = if index_type == Bots::DcaIndex::INDEX_TYPE_CATEGORY && index_category_id.present?
                  @index = Index.find_by(external_id: index_category_id, source: Index::SOURCE_COINGECKO)
                  if @index&.available_exchanges.present?
                    Exchange.available_for_new_bots.where(type: @index.available_exchanges.keys)
                  else
                    Exchange.available_for_new_bots
                  end
                else
                  # "Top N" indices can dynamically find coins on any exchange
                  Exchange.available_for_new_bots
                end

    filter_exchanges_by_query(exchanges: exchanges, query: query)
  end
end
