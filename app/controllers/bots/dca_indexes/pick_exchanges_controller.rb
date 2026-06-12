# Substantially custom step (market-data + index-selected gates, per-exchange
# coin previews, index-aware exchange search) — subclasses the wizard base for
# the shared create flow and overrides the rest wholesale.
class Bots::DcaIndexes::PickExchangesController < Bots::Wizard::PickExchangesController
  before_action :require_market_data_configured
  before_action :require_index_selected

  def new
    session[:bot_config] ||= {}
    prepare_step
    session[:bot_config]['label'] ||= @bot.label
  end

  private

  def bot_relation = current_user.bots.dca_index
  def add_api_key_path = new_bots_dca_indexes_add_api_key_path

  # View state the :new template needs — shared by `new` and `create`'s 422 re-render.
  def prepare_step
    @bot = build_bot
    @bot.exchange_id = nil
    @exchanges = exchange_search_results_for_index_bot(@bot, search_params[:query])
    load_exchange_coins_data
  end

  def require_market_data_configured
    return if MarketData.configured?

    redirect_to new_bots_dca_indexes_setup_coingecko_path
  end

  def require_index_selected
    return if session.dig(:bot_config, 'settings', 'index_type').present?

    redirect_to new_bots_dca_indexes_pick_index_path
  end

  def bot_params
    params.require(:bots_dca_index).permit(:exchange_id)
  end

  def load_exchange_coins_data
    return unless @index.present?

    # Collect all external_ids we need (from per-exchange or global top_coins)
    all_external_ids = if @index.top_coins_by_exchange.present?
                         @index.top_coins_by_exchange.values.flatten.uniq
                       else
                         @index.top_coins || []
                       end

    # Load assets for display
    @assets_by_external_id = Asset.where(external_id: all_external_ids).index_by(&:external_id)

    # For each exchange, get the top coins specific to that exchange
    @exchange_top_coins = {}
    @exchanges.each do |exchange|
      exchange_coin_ids = @index.top_coins_for_exchange(exchange.type)
      coins = exchange_coin_ids.map { |ext_id| @assets_by_external_id[ext_id] }.compact
      @exchange_top_coins[exchange.id] = coins
    end
  end

  def exchange_search_results_for_index_bot(_bot, query)
    index_type = session.dig(:bot_config, 'settings', 'index_type')
    index_category_id = session.dig(:bot_config, 'settings', 'index_category_id')

    @index = nil # Store for view to access coin counts

    exchanges = if index_type == Bots::DcaIndex::INDEX_TYPE_CATEGORY && index_category_id.present?
                  # Resolve regardless of source — category indices come from CoinGecko (crypto) or
                  # the Data API (e.g. the deltabadger-sourced 'nasdaq-100' stock index). external_id
                  # is unambiguous across sources in practice.
                  @index = Index.find_by(external_id: index_category_id)
                  if @index&.available_exchanges.present?
                    Exchange.available.where(type: @index.available_exchanges.keys)
                  else
                    Exchange.available
                  end
                else
                  # Load "Top Coins" index to get per-exchange coin data
                  @index = Index.find_by(external_id: Index::TOP_COINS_EXTERNAL_ID, source: Index::SOURCE_INTERNAL)
                  Exchange.available
                end

    filter_exchanges_by_query(exchanges: exchanges, query: query)
  end
end
