class Bots::DcaIndex < Bot
  include ActionCable::Channel::Broadcasting

  MAX_COINS = 50
  MIN_COINS = 2

  INDEX_TYPE_TOP = 'top'.freeze
  INDEX_TYPE_CATEGORY = 'category'.freeze

  # "Count-named" indices show as "{prefix} {num_coins}" on the user's bot (e.g. a Nasdaq bot
  # trimmed to 7 reads "Nasdaq 7"), while the picker tile shows the full "{prefix} {TOP_N}".
  # Explicit per index_category_id so a thematic index (S&P 500, Layer 1) never degrades to
  # "S&P {num_coins}". Add an entry only for indices that are genuinely top-N ranked.
  COUNT_NAMED_INDEX_PREFIXES = {
    'nasdaq-100' => 'Nasdaq'
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
                 :index_name_prefix  # Count-named label (e.g. "Nasdaq") → "{prefix} {num_coins}"

  validates :quote_amount, presence: true, numericality: { greater_than: 0 }
  validates :num_coins, presence: true, numericality: { greater_than_or_equal_to: MIN_COINS, less_than_or_equal_to: MAX_COINS }
  validates :allocation_flattening, presence: true, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }
  validates :index_type, presence: true, inclusion: { in: [INDEX_TYPE_TOP, INDEX_TYPE_CATEGORY] }
  validate :validate_bot_exchange, if: :exchange_id?, on: :update
  validate :validate_external_ids, on: :update
  validate :validate_unchangeable_assets, on: :update
  validate :validate_unchangeable_interval, on: :update
  validate :validate_unchangeable_exchange, on: :update
  validate :validate_unchangeable_index, on: :update
  validate :validate_market_data_configured, on: :start

  before_validation :clamp_num_coins_to_bounded_index
  before_save :set_tickers, if: :will_save_change_to_exchange_id?

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
  include Bots::DcaIndex::IndexAllocatable
  include Bots::DcaIndex::OrderSetter
  include Bots::DcaIndex::Measurable

  # Shared lifecycle + asset plumbing — keep LAST so the decorator chains above stay on top
  include Bot::Lifecycle
  include Bot::AssetConfigurable # the available_*/ticker queries below override the single-pair defaults

  self.asset_id_setting_keys = %i[quote_asset_id]

  has_many :bot_index_assets, foreign_key: :bot_id, dependent: :destroy
  has_many :index_assets, through: :bot_index_assets, source: :asset

  def parse_params(params)
    {
      quote_asset_id: params[:quote_asset_id].presence&.to_i,
      quote_amount: params[:quote_amount].presence&.to_f,
      interval: params[:interval].presence,
      num_coins: params[:num_coins].presence&.to_i,
      allocation_flattening: params[:allocation_flattening].presence&.to_f
    }.compact
  end

  def execute_action
    update!(status: :executing)

    # Refresh index composition before setting orders
    result = refresh_index_composition
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

  # @param asset_type: :quote_asset (only quote_asset supported for Index bots)
  def available_assets_for_current_settings(asset_type:, include_exchanges: false)
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

    asset_ids = scope.group(:quote_asset_id)
                     .having('COUNT(*) >= ?', Index::ExchangeAvailability::MINIMUM_SUPPORTED_COINS)
                     .pluck(:quote_asset_id)
    include_exchanges ? Asset.includes(:exchanges).where(id: asset_ids) : Asset.where(id: asset_ids)
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

  def decimals
    return {} unless tickers.any?

    @decimals ||= {
      quote: tickers.pluck(:quote_decimals).compact.min
    }
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
      limit: 150
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
      break if preview.size >= MAX_COINS

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

  def display_index_name
    return "#{index_name_prefix} #{num_coins}" if index_name_prefix.present? && num_coins.present?
    return index_name if index_name.present?

    if index_type == INDEX_TYPE_TOP || index_type.blank?
      num_coins.present? ? "Top #{num_coins}" : I18n.t('bot.dca_index.setup.pick_index.top_coins')
    else
      index_category_id&.titleize || 'Index'
    end
  end

  def broadcast_below_minimums_warning
    # Count recent skipped transactions
    recent_transactions = transactions.limit(num_coins.to_i * 2)
    skipped_count = recent_transactions.count(&:skipped?)
    return unless skipped_count.positive? && recent_transactions.count == skipped_count

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'modal',
      partial: 'bots/dca_indexes/warning_below_minimums',
      locals: { bot: self, skipped_count: skipped_count }
    )
  end

  private

  # Server-authoritative cap: a bounded (deltabadger-sourced) index publishes its full
  # universe in top_coins, so num_coins can never exceed it. Runs before validation and
  # before display_index_name, so neither a save nor the bot name can show "Nasdaq 50".
  # Crypto "Top"/coingecko categories are not bounded this way and are left untouched.
  def clamp_num_coins_to_bounded_index
    return if num_coins.blank?

    idx = current_index
    return unless idx&.source == Index::SOURCE_DELTABADGER && idx.top_coins.present?

    self.num_coins = idx.top_coins.size if num_coins.to_i > idx.top_coins.size
  end

  def exchange_supports_current_assets?
    exchange.tickers.available.trading_enabled.exists?(quote_asset_id:)
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
