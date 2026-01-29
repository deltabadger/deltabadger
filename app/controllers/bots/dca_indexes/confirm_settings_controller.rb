class Bots::DcaIndexes::ConfirmSettingsController < ApplicationController
  before_action :authenticate_user!
  before_action :require_coingecko_configured
  before_action :require_bot_config

  def new
    session[:bot_config]['settings'] ||= {}
    session[:bot_config]['settings']['interval'] ||= 'week'
    session[:bot_config]['settings']['quote_amount'] ||= 100
    session[:bot_config]['settings']['num_coins'] ||= 10
    session[:bot_config]['settings']['allocation_flattening'] ||= 0.0
    @bot = current_user.bots.dca_index.new(session[:bot_config])

    # Fetch preview of index composition
    @index_preview = fetch_index_preview(@bot)
  end

  def create
    bot = current_user.bots.dca_index.new(session[:bot_config])
    session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)
    @bot = current_user.bots.dca_index.new(session[:bot_config])

    # Refresh preview with new settings
    @index_preview = fetch_index_preview(@bot)

    return render :create if @bot.quote_amount.blank? || @bot.valid?

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render :create, status: :unprocessable_entity
  end

  private

  def require_coingecko_configured
    return if AppConfig.coingecko_configured?

    redirect_to new_bots_dca_indexes_setup_coingecko_path
  end

  def require_bot_config
    return if session[:bot_config].present? && session.dig(:bot_config, 'exchange_id').present?

    redirect_to new_bots_dca_indexes_pick_index_path
  end

  def bot_params
    params.require(:bots_dca_index).permit(*permitted_settings)
  end

  def permitted_settings
    Bots::DcaIndex.stored_attributes[:settings].reject do |key|
      key.in?(%i[quote_asset_id])
    end
  end

  def fetch_index_preview(bot)
    return [] unless bot.exchange.present? && bot.quote_asset_id.present?

    coingecko = Coingecko.new(api_key: AppConfig.coingecko_api_key)
    # Fetch coins based on index type
    result = if bot.index_type == Bots::DcaIndex::INDEX_TYPE_CATEGORY && bot.index_category_id.present?
               coingecko.get_top_coins_by_category(category: bot.index_category_id, limit: 150)
             else
               coingecko.get_top_coins_by_market_cap(limit: 150)
             end
    return [] if result.failure?

    top_coins = result.data
    available_tickers = bot.exchange.tickers.available.where(quote_asset_id: bot.quote_asset_id).includes(:base_asset)

    ticker_by_coingecko_id = {}
    available_tickers.each do |ticker|
      next unless ticker.base_asset&.external_id.present?

      ticker_by_coingecko_id[ticker.base_asset.external_id] = ticker
    end

    # Collect up to MAX_COINS for live preview (allocations calculated client-side)
    preview = []
    top_coins.each do |coin|
      break if preview.size >= Bots::DcaIndex::MAX_COINS

      ticker = ticker_by_coingecko_id[coin['id']]
      next unless ticker.present?

      preview << {
        symbol: ticker.base_asset.symbol,
        name: ticker.base_asset.name,
        color: ticker.base_asset.color,
        market_cap: coin['market_cap'].to_f,
        rank: preview.size + 1
      }
    end

    preview
  end
end
