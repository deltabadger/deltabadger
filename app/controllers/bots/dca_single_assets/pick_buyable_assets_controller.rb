# The DCA wizard's asset step, shared by the single- and multi-asset bots. Assets are collected
# here — picking stays on the step — and the bot type is decided only when the user leaves it:
# one asset continues as a DcaSingleAsset, two or more as a DcaMultiAsset. The URL stays in the
# single namespace, which also hosts the exchange/API steps that precede the decision in the
# exchange-first order.
#
# The session shape follows the count: one asset → settings.base_asset_id (what every single step
# understands), two or more → settings.base_asset_ids. Reads tolerate either — and anything stale —
# and every POST out of the step rewrites the canonical shape. GET never writes: Turbo prefetches
# it on hover.
class Bots::DcaSingleAssets::PickBuyableAssetsController < ApplicationController
  before_action :authenticate_user!
  before_action :redirect_if_session_expired, only: %i[remove advance]

  include Bots::Searchable
  include Bots::WizardSessionGuard
  include Bots::Wizard::Navigable
  include Bots::StockBrokerRoutable

  def new
    session[:bot_config] ||= {}
    @bot = build_bot # the prerequisite predicate reads its api_key
    if (path = prerequisite_redirect_path) then return redirect_to path end

    prepare_step
    render_asset_page(bot: @bot, asset_field: :base_asset_id)
  end

  # Adds one asset and stays. Only an asset drawn from the current search scope may join, so a
  # crafted request cannot smuggle in an unsupported pair; duplicates and the twenty-first are
  # ignored. A first pick is self-sufficient (no session yet), like every first step.
  def create
    session[:bot_config] ||= {}
    id = bot_params[:base_asset_id].to_i
    if id.zero?
      prepare_step
      return render :new, status: :unprocessable_entity
    end

    ids = chosen_asset_ids
    if !ids.include?(id) && ids.size < Bots::DcaMultiAsset::MAX_ASSETS &&
       search_scope.available_assets_for_current_settings(asset_type: :base_asset).exists?(id)
      delete_session_path(Bots::Wizard::StepOrder::EXCHANGE_KEY) if fresh_start?
      write_ids(ids + [id])
    end
    redirect_to new_bots_dca_single_assets_pick_buyable_asset_path
  end

  # The basket table comes back open: the user was working in it.
  def remove
    write_ids(chosen_asset_ids - [bot_params[:base_asset_id].to_i])
    redirect_to new_bots_dca_single_assets_pick_buyable_asset_path(basket: 'open')
  end

  # The one way out of the step (Next, the sentence placeholders, and the exchange chip with
  # `to=exchange`), so the basket is committed in its canonical shape before any later step reads
  # it. Next goes to the first step of the decided order that is still missing input — a fresh run
  # lands on the exchange (asset-first) or the quote (exchange-first); coming back from a later
  # step with the exchange and key in place lands on the quote instead of re-asking.
  def advance
    ids = chosen_asset_ids
    multi = multi_basket?

    if params[:to] == 'exchange'
      write_ids(ids)
      return redirect_to step_path(:exchange)
    end

    return redirect_to new_bots_dca_single_assets_pick_buyable_asset_path if ids.empty?

    if eligible_exchanges_for(basket).empty?
      flash[:alert] = t('bot.dca_multi_asset.no_common_exchange')
      return redirect_to new_bots_dca_single_assets_pick_buyable_asset_path
    end

    write_ids(ids)
    @bot = build_bot
    decided = multi ? basket : @bot
    if asset_first? && stock_bot? && !broker_chosen?(decided)
      route_to_stock_broker(decided, multi:)
    elsif multi
      order = Bots::Wizard::StepOrder.for(bot_type: :multi, variant: current_variant)
      redirect_to step_path(first_incomplete_after(order, :assets))
    else
      redirect_to step_path(first_incomplete_after(current_order, :currencies))
    end
  end

  private

  def current_step = :currencies
  def bot_relation = current_user.bots.dca_single_asset
  def build_bot = bot_relation.new(sanitized_bot_config)

  # Paths follow the basket: a basket of two or more belongs to the multi namespace, whichever step
  # is asked for — the prerequisite bounce in `new` included (a 2+ basket whose key turned invalid
  # must land on the multi API step, whose exchange back-link keeps the list).
  def step_path(key)
    return new_bots_dca_single_assets_pick_buyable_asset_path if key.in?(%i[currencies assets])

    multi_basket? ? multi_step_path(key) : single_step_path(key)
  end

  def single_step_path(key)
    case key
    when :exchange  then new_bots_dca_single_assets_pick_exchange_path
    when :api       then new_bots_dca_single_assets_add_api_key_path
    when :spendable then new_bots_dca_single_assets_pick_spendable_asset_path
    end
  end

  def multi_step_path(key)
    case key
    when :exchange  then new_bots_dca_multi_assets_pick_exchange_path
    when :api       then new_bots_dca_multi_assets_add_api_key_path
    when :spendable then new_bots_dca_multi_assets_pick_spendable_asset_path
    end
  end

  def multi_basket? = chosen_assets.size >= Bots::DcaMultiAsset::MIN_ASSETS

  # In asset-first an empty basket is a fresh start: an exchange left behind by another wizard, or
  # by emptying the basket, is neither shown nor searched against, and the first pick drops it. In
  # exchange-first the venue was chosen first, on purpose, and stays.
  def fresh_start? = asset_first? && chosen_assets.empty?

  # step_complete?(:exchange/:api/:spendable) reads the same session keys in both orders, so the
  # single and multi branches share it; only the asset step itself is keyed differently.
  def first_incomplete_after(order, step)
    rest = order.steps.drop(order.steps.index(step) + 1)
    rest.find { |key| !step_complete?(key) } || order.steps.last
  end

  def bot_params
    params.require(:bots_dca_single_asset).permit(:base_asset_id)
  end

  # The chosen assets in pick order, from whichever shape the session holds, minus anything that
  # cannot be a member (blank, duplicate, or no longer an Asset row).
  def chosen_assets
    @chosen_assets ||= begin
      settings = session.dig(:bot_config, 'settings') || {}
      raw = Array(settings['base_asset_ids'].presence || settings['base_asset_id']).map(&:to_i).reject(&:zero?).uniq
      by_id = Asset.where(id: raw).index_by(&:id)
      raw.filter_map { |id| by_id[id] }
    end
  end

  def chosen_asset_ids = chosen_assets.map(&:id)
  def stock_bot? = chosen_assets.any? { |asset| asset.category == 'Stock' }

  # Every wizard key but the exchange goes (both base shapes, the legacy dual base0/base1 scrub and
  # the quote — the step owns everything from the quote onward), then the one key matching the
  # count is written. The exchange stays: its chip keeps showing, eligibility narrows to it.
  def write_ids(ids)
    session[:bot_config] ||= {}
    (Bots::Wizard::StepOrder::ALL_WIZARD_KEYS - [Bots::Wizard::StepOrder::EXCHANGE_KEY]).each { |path| delete_session_path(path) }
    settings = (session[:bot_config]['settings'] ||= {})
    case ids.size
    when 0 then nil
    when 1 then settings['base_asset_id'] = ids.first
    else settings['base_asset_ids'] = ids
    end
  end

  # A DcaMultiAsset holding the basket gives the list semantics — only assets sharing a venue and
  # quote with the chosen ones, venues carrying all of them — for one asset as well as twenty. It
  # never sees the session quote, so the page and `advance` judge one basket.
  def basket
    @basket ||= current_user.bots.dca_multi_asset.new(
      exchange_id: session.dig(:bot_config, 'exchange_id'),
      settings: { 'base_asset_ids' => chosen_asset_ids }
    )
  end

  # Before the first pick the single bot's own scope lists the catalogue (the multi model would
  # materialise every venue/quote pair into one OR query for an empty basket).
  def search_scope = chosen_assets.empty? ? blank_single_bot : basket

  def blank_single_bot
    bot = build_bot
    bot.base_asset_id = nil
    bot.quote_asset_id = nil
    bot.exchange_id = nil if fresh_start?
    bot
  end

  def prepare_step
    @bot = blank_single_bot
    @chosen = chosen_assets
    @eligible_exchanges = @chosen.any? ? eligible_exchanges_for(basket) : Exchange.none
    if @chosen.size >= Bots::DcaMultiAsset::MAX_ASSETS
      # The lazy renderer expects an offset even though the capped list has no search page.
      @assets = []
      @asset_page_offset = 0
    else
      @assets = asset_search_results(search_scope, params[:query], :base_asset)
    end
  end

  def eligible_exchanges_for(bot)
    exchanges = bot.available_exchanges_for_current_settings
    bot.exchange_id? ? exchanges.where(id: bot.exchange_id) : exchanges
  end

  # A broker already chosen for this basket (back-navigation from a later step) is kept;
  # redirect_after_stock_asset would otherwise clear it and reopen the picker.
  def broker_chosen?(bot)
    bot.exchange_id.present? && available_stock_brokers(bot).any? { |venue| venue.id == bot.exchange_id.to_i }
  end

  def route_to_stock_broker(bot, multi:)
    if multi
      # The routing concern is deliberately silent when no venue qualifies; the user needs the
      # reason they remained on the asset step.
      flash[:alert] = t('bot.dca_multi_asset.no_common_exchange') if available_stock_brokers(bot).empty?
      redirect_after_stock_asset(
        bot,
        picker_path: new_bots_dca_multi_assets_pick_stock_broker_path,
        add_api_key_path: new_bots_dca_multi_assets_add_api_key_path,
        repick_path: new_bots_dca_single_assets_pick_buyable_asset_path
      )
    else
      redirect_after_stock_asset(
        bot,
        picker_path: new_bots_dca_single_assets_pick_stock_broker_path,
        add_api_key_path: new_bots_dca_single_assets_add_api_key_path,
        repick_path: new_bots_dca_single_assets_pick_buyable_asset_path
      )
    end
  end
end
