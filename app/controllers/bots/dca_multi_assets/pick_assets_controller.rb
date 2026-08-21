class Bots::DcaMultiAssets::PickAssetsController < ApplicationController
  before_action :authenticate_user!
  before_action :redirect_if_session_expired, only: %i[create remove advance]

  include Bots::Searchable
  include Bots::WizardSessionGuard
  include Bots::DcaMultiAssets::WizardSteps
  include Bots::StockBrokerRoutable

  # Turbo prefetches GETs. An empty list is restored to the single flow before redirecting so the
  # single picker cannot inherit the multi label.
  def new
    return restart_as_single! if chosen_asset_ids.empty?
    if (path = prerequisite_redirect_path) then return redirect_to path end

    prepare_step
    render_asset_page(bot: @bot, asset_field: :base_asset_id)
  end

  # Only assets drawn from this bot's own search may join, so crafted requests cannot smuggle in an
  # unsupported pair.
  def create
    id = params.dig(:bots_dca_multi_asset, :base_asset_id).to_i
    ids = chosen_asset_ids
    bot = build_bot
    if id.positive? && !ids.include?(id) && ids.size < Bots::DcaMultiAsset::MAX_ASSETS &&
       bot.available_assets_for_current_settings(asset_type: :base_asset).exists?(id)
      write_ids(ids + [id])
      delete_session_path(Bots::Wizard::StepOrder::QUOTE_KEY)
    end
    redirect_to new_bots_dca_multi_assets_pick_assets_path
  end

  def remove
    ids = chosen_asset_ids - [params.dig(:bots_dca_multi_asset, :base_asset_id).to_i]
    if ids.size <= 1
      demote_to_single!(ids.first)
    else
      write_ids(ids)
      delete_session_path(Bots::Wizard::StepOrder::QUOTE_KEY)
      redirect_to new_bots_dca_multi_assets_pick_assets_path
    end
  end

  def advance
    return redirect_to new_bots_dca_multi_assets_pick_assets_path if chosen_asset_ids.size < Bots::DcaMultiAsset::MIN_ASSETS

    @bot = build_bot
    eligible_exchanges = eligible_exchanges_for(@bot)
    if eligible_exchanges.empty?
      flash[:alert] = t('bot.dca_multi_asset.no_common_exchange')
      return redirect_to new_bots_dca_multi_assets_pick_assets_path
    end

    if asset_first? && Asset.where(id: chosen_asset_ids, category: 'Stock').exists?
      # The routing concern is deliberately silent when no venue qualifies; the user needs the
      # reason they remained on the asset step.
      flash[:alert] = t('bot.dca_multi_asset.no_common_exchange') if available_stock_brokers(@bot).empty?
      redirect_after_stock_asset(
        @bot,
        picker_path: new_bots_dca_multi_assets_pick_stock_broker_path,
        add_api_key_path: new_bots_dca_multi_assets_add_api_key_path,
        repick_path: new_bots_dca_multi_assets_pick_assets_path
      )
    else
      advance!
    end
  end

  private

  def current_step = :assets
  def build_bot = bot_relation.new(sanitized_bot_config)
  def write_ids(ids) = session[:bot_config].deep_merge!('settings' => { 'base_asset_ids' => ids })

  def prepare_step
    @bot = build_bot
    by_id = Asset.where(id: chosen_asset_ids).index_by(&:id)
    @chosen = chosen_asset_ids.filter_map { |id| by_id[id] }
    @eligible_exchanges = eligible_exchanges_for(@bot)
    if chosen_asset_ids.size >= Bots::DcaMultiAsset::MAX_ASSETS
      # The lazy renderer expects an offset even though the capped list has no search page.
      @assets = []
      @asset_page_offset = 0
    else
      @assets = asset_search_results(@bot, params[:query], :base_asset)
    end
  end

  def eligible_exchanges_for(bot)
    exchanges = bot.available_exchanges_for_current_settings
    bot.exchange_id? ? exchanges.where(id: bot.exchange_id) : exchanges
  end

  # One remaining asset belongs to the single type; its prerequisite guard finds the first gap.
  def demote_to_single!(base_id)
    cfg = session[:bot_config] || {}
    session[:bot_config] = {
      'label' => Bots::DcaSingleAsset.new.label,
      'flow' => cfg['flow'],
      'exchange_id' => cfg['exchange_id'],
      'settings' => { 'base_asset_id' => base_id }.compact
    }.compact
    redirect_to new_bots_dca_single_assets_pick_spendable_asset_path
  end

  def restart_as_single!
    cfg = session[:bot_config] || {}
    session[:bot_config] = {
      'label' => Bots::DcaSingleAsset.new.label,
      'flow' => cfg['flow'],
      'exchange_id' => cfg['exchange_id'],
      'settings' => {}
    }.compact
    redirect_to new_bots_dca_single_assets_pick_buyable_asset_path
  end
end
