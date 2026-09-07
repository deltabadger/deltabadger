class Bots::DcaIndex < Bot
  include ActionCable::Channel::Broadcasting

  MAX_COINS = 50
  MIN_COINS = 2

  INDEX_TYPE_TOP = 'top'.freeze
  INDEX_TYPE_CATEGORY = 'category'.freeze

  # "Count-named" indices show as "{prefix}{num_coins}" on the user's bot (an ND100 bot trimmed to
  # seven reads "ND7") and as the index's own name once the bot holds the whole universe. The map is
  # the source of the name — NOT the feed's Index#name — so the product's wording never depends on
  # what a data provider happens to publish. Explicit per index_category_id so a thematic index
  # never degrades to "S&P{num_coins}". Add an entry only for indices that are genuinely top-N ranked.
  COUNT_NAMED_INDICES = {
    'nasdaq-100' => { prefix: 'ND', name: 'ND100' }.freeze
  }.freeze

  store_accessor :settings,
                 :quote_asset_id,
                 :quote_amount,
                 :interval,
                 :num_coins,
                 :allocation_flattening,
                 :index_type,        # 'top' or 'category'
                 :index_category_id, # CoinGecko category ID (when index_type is 'category')
                 :index_name,        # Cached display name for the index
                 :index_name_prefix, # Count-named label (e.g. "ND") → "{prefix}{num_coins}"
                 :hold_all           # Intent: hold the whole universe of a bounded index

  validates :quote_amount, presence: true, numericality: { greater_than: 0 }
  validates :num_coins, presence: true,
                        numericality: { greater_than_or_equal_to: MIN_COINS, less_than_or_equal_to: :validation_max_coins }
  validates :allocation_flattening, presence: true, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }
  validates :index_type, presence: true, inclusion: { in: [INDEX_TYPE_TOP, INDEX_TYPE_CATEGORY] }
  # Both contexts, one registration: Rails dedupes validation callbacks by filter symbol, so a
  # second `validate :validate_bot_exchange` would silently replace this one rather than add to it.
  validate :validate_bot_exchange, if: :exchange_id?, on: %i[update start]
  validate :validate_external_ids, on: :update
  validate :validate_unchangeable_assets, on: :update
  validate :validate_unchangeable_interval, on: :update
  validate :validate_unchangeable_exchange, on: :update
  validate :validate_unchangeable_index, on: :update
  validate :validate_market_data_configured, on: :start

  before_validation :clamp_num_coins_to_bounded_index
  before_save :set_tickers, if: :will_save_change_to_exchange_id?

  # The composition is DERIVED from these, and used to be re-derived only at the next buy or
  # rebalance. So moving the coins slider redrew the donut — which is live, straight from the
  # preview — and left both tables under it describing the old index: a coin the slider had just
  # taken back in went on sitting under "Left the index" with a Sell button over it, for up to a
  # whole interval. Re-derive it now, then push the tables the split they now belong on.
  after_update_commit :resync_index_composition,
                      if: -> { index_definition_changed? || custom_exchange_id_changed? }

  # Trading condition concerns (only SmartIntervalable and LimitOrderable for Index bot)
  include SmartIntervalable      # decorators for: parse_params, effective_quote_amount, effective_interval_duration
  include LimitOrderable         # decorators for: parse_params, execute_action

  # Standard infrastructure concerns
  include Fundable # decorators for: execute_action
  include Automation::Schedulable
  include Bot::Startable # decorators for: parse_params; overrides Schedulable defaults — keep AFTER Schedulable
  include OrderCreator
  include Accountable
  include Exportable

  # Type-specific concerns
  include Bot::Rebalanceable # decorators for: parse_params — keep AFTER the limitables
  include Bot::Composition::Allocatable
  include Bots::DcaIndex::IndexAllocatable
  include Bot::Composition::OrderSetter
  include Bot::Composition::Rebalancer
  include Bot::Composition::Liquidatable
  include Bot::Composition::Redeployable
  include Bot::Composition::Measurable

  # Shared lifecycle + asset plumbing — keep LAST so the decorator chains above stay on top
  include Bot::Lifecycle
  include Bot::AssetConfigurable # the available_*/ticker queries below override the single-pair defaults

  self.asset_id_setting_keys = %i[quote_asset_id]

  def parse_params(params)
    {
      quote_asset_id: params[:quote_asset_id].presence&.to_i,
      quote_amount: params[:quote_amount].presence&.to_f,
      interval: params[:interval].presence,
      num_coins: slider_moved?(params) ? params[:num_coins].to_i : nil,
      hold_all: hold_all_from(params),
      allocation_flattening: params[:allocation_flattening].presence&.to_f
    }.compact
  end

  # The settings form says what the slider was RENDERED with (num_coins_rendered) beside what it
  # submits; the slider was moved only if the two differ. The wizard and the API send no rendered
  # value, so a submitted count there always counts as moved. This matters because the settings
  # slider is clamped to what the venue lists right now: on a day a ticker is missing locally a
  # fixed ND20 renders as 19 of 19, and an unrelated save must neither store 19 nor read "19 of a
  # 19 ceiling" as the intent to hold all.
  def slider_moved?(params)
    return false if params[:num_coins].blank?

    rendered = params[:num_coins_rendered].presence
    rendered.nil? || params[:num_coins].to_i != rendered.to_i
  end

  # "All of it" is intent the user expresses by moving the slider to its end; an untouched slider
  # says nothing (nil, compacted away — the flag stays as it was). A moved slider sets the flag by
  # whether it sits on the ceiling the form rendered (num_coins_ceiling; the API sends none and is
  # judged against the universe).
  def hold_all_from(params)
    return nil unless slider_moved?(params)

    params[:num_coins].to_i >= (params[:num_coins_ceiling].presence || max_coins).to_i
  end

  def execute_action
    update!(status: :executing)

    # Orders must use the composition derived for this tick.
    result = refresh_composition
    return result if result.failure?

    result = set_orders(
      total_orders_amount_in_quote: pending_quote_amount,
      update_missed_quote_amount: true
    )
    return result if result.failure?

    update!(status: :waiting)
    broadcast_below_minimums_warning
    Result::Success.new
  end

  def available_exchanges_for_current_settings
    scope = Ticker.available.trading_enabled.where(exchange: Exchange.available)
    scope = scope.where(quote_asset_id:) if quote_asset_id.present?
    exchange_ids = scope.pluck(:exchange_id).uniq
    Exchange.where(id: exchange_ids)
  end

  # Index bots have no base_asset_id of their own, so they cannot share the single-pair
  # offered_tickers; they narrow by the index's member coins instead. Same contract: the step's
  # rows and its exchange logos are two plucks off this one relation.
  #
  # @param asset_type: :quote_asset (only quote_asset supported for Index bots)
  def offered_tickers(asset_type: :quote_asset)
    available_exchanges = exchange.present? ? [exchange] : Exchange.available
    scope = Ticker.available.trading_enabled.where(exchange: available_exchanges)

    # For category index bots, only show quote assets that have enough pairs with category coins
    if index_type == INDEX_TYPE_CATEGORY && index_category_id.present? && exchange.present?
      index = Index.find_by(external_id: index_category_id)
      if index.present?
        coin_ids = index.top_coins_for_exchange(exchange.type)
        base_asset_ids = Asset.where(external_id: coin_ids).pluck(:id) if coin_ids.present?
        scope = scope.where(base_asset_id: base_asset_ids) if base_asset_ids&.any?
      end
    end

    scope
  end

  # @param asset_type: :quote_asset (only quote_asset supported for Index bots)
  def available_assets_for_current_settings(asset_type:)
    asset_ids = offered_tickers(asset_type:)
                .group(:quote_asset_id)
                .having('COUNT(*) >= ?', Index::ExchangeAvailability::MINIMUM_SUPPORTED_COINS)
                .pluck(:quote_asset_id)
    Asset.where(id: asset_ids)
  end

  # Returns { quote_asset_id => [Asset, Asset, ...] } for the bot's current
  # index + exchange combo. Each list is sorted by base-asset market-cap rank
  # (highest market cap first). Used by the quote-currency picker to render
  # the same ticker-group preview that the index tiles show on step 2.
  def top_base_assets_by_quote_for_current_setup
    return {} if exchange.blank?

    index = current_index
    return {} if index.blank?

    external_ids = index.top_coins_for_exchange(exchange.type)
    return {} if external_ids.blank?

    base_assets = Asset.where(external_id: external_ids).index_by(&:id)
    return {} if base_assets.empty?

    Ticker.available.trading_enabled
          .where(exchange_id: exchange.id, base_asset_id: base_assets.keys)
          .joins(:base_asset)
          .order(Arel.sql('assets.market_cap_rank IS NULL'), Arel.sql('assets.market_cap_rank ASC'))
          .pluck(:quote_asset_id, :base_asset_id)
          .each_with_object({}) do |(quote_id, base_id), acc|
            acc[quote_id] ||= []
            acc[quote_id] << base_assets[base_id]
          end
  end

  # Resolves the Index record for the bot's current settings. Categories look
  # up by `index_category_id`; the "Top" type uses the internal Top Coins index.
  def current_index
    if index_type == INDEX_TYPE_CATEGORY && index_category_id.present?
      Index.find_by(external_id: index_category_id)
    else
      Index.find_by(external_id: Index::TOP_COINS_EXTERNAL_ID, source: Index::SOURCE_INTERNAL)
    end
  end

  def quote_asset
    @quote_asset ||= Asset.find_by(id: quote_asset_id)
  end

  # Memoized WHOLE, guard included: `tickers` is an unloaded relation, so `any?` is an EXISTS
  # query every call — and the chart reads this once per data point, which turned one render
  # into 1457 identical queries.
  def decimals
    @decimals ||= tickers.any? ? { quote: tickers.pluck(:quote_decimals).compact.min } : {}
  end

  # Returns the highest minimum_quote_size among all index tickers.
  # Ensures smart interval amount is high enough that at least one order can execute,
  # even if 100% of funds concentrate on a single coin during rebalancing.
  def minimum_for_exchange
    return 0 unless tickers.any?

    tickers.maximum(:minimum_quote_size).to_f
  end

  def current_index_preview
    return [] unless exchange.present? && quote_asset_id.present?

    result = MarketData.get_top_coins(
      index_type: index_type,
      category_id: index_category_id,
      limit: bounded_universe_size || 150
    )
    return [] if result.failure?

    top_coins = result.data
    available_tickers = exchange.tickers.available.trading_enabled.where(quote_asset_id: quote_asset_id).includes(:base_asset)

    ticker_by_coingecko_id = {}
    available_tickers.each do |ticker|
      next unless ticker.base_asset&.external_id.present?

      ticker_by_coingecko_id[ticker.base_asset.external_id] = ticker
    end

    preview = []
    top_coins.each do |coin|
      break if preview.size >= max_coins

      ticker = ticker_by_coingecko_id[coin['id']]
      next unless ticker.present?

      preview << {
        asset_id: ticker.base_asset.id,
        symbol: ticker.base_asset.symbol,
        name: ticker.base_asset.name,
        color: ticker.base_asset.color,
        logo: ticker.base_asset.image_url,
        market_cap: coin['market_cap'].to_f,
        rank: preview.size + 1
      }
    end

    preview
  end

  # "Layer 1 · 20". A count-named index ("ND7") and a Top-N ("Top 10") already carry their
  # size in the name, so only a thematic category has the coin count appended.
  def default_label
    # The name must quote the count the bot will actually buy, and the clamp that decides it runs
    # later in the same validation.
    clamp_num_coins_to_bounded_index
    return display_index_name if COUNT_NAMED_INDICES.key?(index_category_id) || index_type != INDEX_TYPE_CATEGORY

    [display_index_name, num_coins].compact.join(' · ')
  end

  def display_index_name
    if (named = COUNT_NAMED_INDICES[index_category_id]) && effective_num_coins.present?
      return holds_whole_universe? ? named[:name] : "#{named[:prefix]}#{effective_num_coins}"
    end
    return index_name if index_name.present?

    if index_type == INDEX_TYPE_TOP || index_type.blank?
      num_coins.present? ? "Top #{num_coins}" : I18n.t('bot.dca_index.setup.pick_index.top_coins')
    else
      index_category_id&.titleize || 'Index'
    end
  end

  # Size of a bounded (deltabadger-sourced) index's published universe; nil for a crypto Top or
  # CoinGecko category index, which publish a ranking rather than a membership. Memoised PER
  # CATEGORY ID, not per instance: after_initialize asks for it before a factory or the wizard has
  # assigned the settings, and a nil remembered then would be the answer forever. Dropped on
  # reload, so a test (or a long-lived job) that reloads the bot after the index changed sees the
  # new size.
  def bounded_universe_size
    @bounded_universe_sizes ||= {}
    key = index_category_id.to_s
    return @bounded_universe_sizes[key] if @bounded_universe_sizes.key?(key)

    idx = current_index
    @bounded_universe_sizes[key] = idx&.source == Index::SOURCE_DELTABADGER && idx.top_coins.present? ? idx.top_coins.size : nil
  end

  def reload(...)
    @bounded_universe_sizes = nil
    super
  end

  # The slider's ceiling. A bounded index can be held whole, so its ceiling is its own size; the
  # crypto Top and category indices keep the fixed cap (their ranking has no natural end).
  def max_coins
    bounded_universe_size || MAX_COINS
  end

  # "Hold the whole universe" is intent (Decision 17), so it survives the universe growing or a
  # one-day shrink. A bot that was saved below the ceiling keeps its count — and a bot whose count
  # merely EQUALS today's universe (an ND20 bot while the feed still publishes twenty) is not
  # holding the whole universe, it is holding twenty.
  def holds_whole_universe?
    hold_all? && bounded_universe_size.to_i.positive?
  end

  # What the composition is derived at and what the name quotes.
  def effective_num_coins
    return bounded_universe_size if holds_whole_universe?

    num_coins
  end

  def hold_all?
    ActiveModel::Type::Boolean.new.cast(hold_all)
  end

  def composition_size = effective_num_coins.to_i
  def exited_title_key = 'bot.dca_index.left_the_index'
  def metrics_partial = 'bots/composition/metrics'

  private

  # Server-authoritative cap: a bounded (deltabadger-sourced) index publishes its full
  # universe in top_coins, so num_coins can never exceed it. Runs before validation and
  # before display_index_name, so neither a save nor the bot name can show a count it cannot buy.
  # Crypto "Top"/coingecko categories are not bounded this way and are left untouched.
  def clamp_num_coins_to_bounded_index
    return if num_coins.blank? || bounded_universe_size.nil?
    # A bot that holds the whole universe keeps the count it was saved with. Rewriting it on a
    # one-day shrink would dirty the settings of a save that never touched them — which
    # Bot::Accountable refuses outright, leaving the bot unable even to mark itself executing — and
    # what such a bot trades is effective_num_coins, the live universe size, not this number.
    return if holds_whole_universe?

    self.num_coins = bounded_universe_size if num_coins.to_i > bounded_universe_size
  end

  # The ceiling the record is judged against. Same as the slider's, except for a bot holding the
  # whole universe: its stored count is the size the universe had when it was saved, and a shrink
  # must not turn the bot invalid.
  def validation_max_coins
    holds_whole_universe? ? [max_coins, num_coins.to_i].max : max_coins
  end

  def exchange_supports_current_assets?
    exchange.tickers.available.trading_enabled.exists?(quote_asset_id:)
  end

  # Everything the derivation reads to decide MEMBERSHIP. limit_ordered belongs here because it
  # picks which side a candidate has to quote on to count (index_allocatable.rb: priced?(:last)
  # against priced?(:ask)). allocation_flattening does not: it moves the WEIGHTS, and both tables
  # read holdings, not targets — the rebalancer re-derives its own targets before it acts.
  INDEX_DEFINITION_KEYS = %w[num_coins hold_all index_type index_category_id quote_asset_id limit_ordered].freeze

  # Not saved_change_to_settings?: with store_accessor the settings column is written on every save
  # whether or not a value moved (see Automation::Configurable), so the keys have to be compared.
  def index_definition_changed?
    before, after = saved_changes['settings']
    before.present? && INDEX_DEFINITION_KEYS.any? { |key| before[key] != after[key] }
  end

  def resync_index_composition
    Bot::ResyncIndexCompositionJob.perform_later(self)
  end

  def validate_unchangeable_index
    return unless settings_changed?
    return unless transactions.any?

    return unless index_type_was != index_type || index_category_id_was != index_category_id

    errors.add(:index_type, :unchangeable,
               message: I18n.t('errors.bots.index_change_after_transactions'))
  end

  def validate_market_data_configured
    return if MarketData.configured?

    errors.add(:base, :market_data_required,
               message: I18n.t('errors.bots.market_data_required'))
  end

  def set_tickers
    return Ticker.none unless exchange.present?

    @tickers = exchange.tickers.available.trading_enabled.where(quote_asset_id:)
  end
end
