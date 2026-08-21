class Bots::DcaMultiAsset < Bot
  include ActionCable::Channel::Broadcasting

  MIN_ASSETS = 2
  MAX_ASSETS = 20

  store_accessor :settings, :quote_asset_id, :quote_amount, :interval, :allocations, :base_asset_ids

  validates :quote_amount, presence: true, numericality: { greater_than: 0 }
  validate :validate_external_ids, on: :update
  # asset_id_setting_keys is only the quote: once the bot has bought anything, every ledger figure is
  # denominated in it, and a swap would mix currencies in invested value and P/L for good.
  validate :validate_unchangeable_assets, on: :update
  validate :validate_unchangeable_interval, on: :update
  validate :validate_unchangeable_exchange, on: :update
  validate :validate_tickers_available, on: :start

  before_save :set_tickers, if: :will_save_change_to_exchange_id?
  # Derivation is database-only, so reconcile synchronously and never expose stale composition rows.
  after_save :refresh_composition,
             if: -> { saved_change_to_id? || composition_changed? || saved_change_to_exchange_id? }
  after_update_commit :broadcast_metrics_panel,
                      if: -> { composition_changed? || saved_change_to_exchange_id? }

  # Trading condition concerns (only SmartIntervalable and LimitOrderable for composition bots)
  include SmartIntervalable
  include LimitOrderable

  # Standard infrastructure concerns
  include Fundable
  include Automation::Schedulable
  include Bot::Startable
  include OrderCreator
  include Accountable
  include Exportable

  # Type-specific concerns
  include Bot::Rebalanceable
  include Bot::Composition::Allocatable
  include Bots::DcaMultiAsset::Allocatable
  include Bot::Composition::OrderSetter
  include Bot::Composition::Rebalancer
  include Bot::Composition::Liquidatable
  include Bot::Composition::Measurable

  # Shared lifecycle + asset plumbing stay last so the decorator chains above remain on top.
  include Bot::Lifecycle
  include Bot::AssetConfigurable

  self.asset_id_setting_keys = %i[quote_asset_id]

  COMPOSITION_KEYS = %w[allocations quote_asset_id].freeze

  def parse_params(params)
    parsed = {
      quote_asset_id: params[:quote_asset_id].presence&.to_i,
      quote_amount: params[:quote_amount].presence&.to_f,
      interval: params[:interval].presence,
      allocations: parse_allocations(params[:allocations])
    }.compact

    # Structural edits apply in order on top of sliders posted in the same request, so one submit
    # cannot throw away another control's change.
    base = parsed[:allocations] || allocations
    base = allocations_adding(params[:add_asset_id].to_i, base) if params[:add_asset_id].present?
    base = allocations_removing(params[:remove_asset_id].to_i, base) if params[:remove_asset_id].present?
    normalize = params[:normalize_allocations].to_s.in?(%w[1 true])
    base = normalize_allocations(base) if normalize
    structural_edit = params[:add_asset_id].present? || params[:remove_asset_id].present? || normalize
    parsed[:allocations] = base if structural_edit
    parsed
  end

  def execute_action
    update!(status: :executing)

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

  # The exchange picker must preserve (exchange, quote) pairs. A venue with A/USD and B/EUR is not
  # a home for an A+B basket even though it lists both assets.
  def eligible_pairs(venues:)
    scope = Ticker.available.trading_enabled.where(exchange: venues)
    scope = scope.where(quote_asset_id:) if quote_asset_id.present?
    return scope.distinct.pluck(:exchange_id, :quote_asset_id) if base_asset_ids.empty?

    scope.where(base_asset_id: base_asset_ids)
         .group(:exchange_id, :quote_asset_id)
         .having('COUNT(DISTINCT base_asset_id) = ?', base_asset_ids.size)
         .pluck(:exchange_id, :quote_asset_id)
  end

  def tickers_on(pairs)
    return Ticker.none if pairs.empty?

    pairs.map do |exchange_id, quote_id|
      Ticker.where(exchange_id:, quote_asset_id: quote_id)
    end.reduce(:or).merge(Ticker.available.trading_enabled)
  end

  def available_exchanges_for_current_settings
    Exchange.where(id: eligible_pairs(venues: Exchange.tradeable).map(&:first).uniq)
  end

  def available_assets_for_current_settings(asset_type:, include_exchanges: false)
    venues = exchange_id? ? Exchange.tradeable.where(id: exchange_id) : Exchange.tradeable
    scope = tickers_on(eligible_pairs(venues:))
    case asset_type
    when :base_asset
      scope = scope.where.not(base_asset_id: base_asset_ids + [quote_asset_id].compact)
    when :quote_asset
      scope = scope.where(base_asset_id: base_asset_ids) if base_asset_ids.any?
    end
    asset_ids = scope.pluck("#{asset_type}_id").uniq
    include_exchanges ? Asset.includes(:exchanges).where(id: asset_ids) : Asset.where(id: asset_ids)
  end

  def assets
    [quote_asset, *base_assets].compact
  end

  def quote_asset
    @quote_asset ||= Asset.find_by(id: quote_asset_id)
  end

  # Empty, like the index bot's. Bot::RebalanceJob#resumable? pre-checks this list BEFORE
  # before_rebalance can refresh the composition, so a delisted member here would wedge every poll
  # and strand a sold leg's proceeds. The per-asset filter in Bot::Rebalancer and
  # validate_tickers_available on :start are the real checks.
  def tickers_for_start = []

  # Memoized whole, guard included: the chart calls this once per data point.
  def decimals
    @decimals ||= tickers.any? ? { quote: tickers.pluck(:quote_decimals).compact.min } : {}
  end

  def minimum_for_exchange
    composition_tickers.filter_map(&:minimum_quote_size).max.to_f
  end

  def composition_size = base_asset_ids.size
  def exited_title_key = 'bot.dca_multi_asset.removed_from_portfolio'
  def metrics_partial = 'bots/composition/metrics'

  # An unconfirmed total must not steer a stopped bot toward proportions the user has not accepted.
  # A swap already in flight still needs its target so its owed buy can land.
  def rebalance_targets
    rebalance_pending? || allocations_balanced? ? super : nil
  end

  private

  # Slider values are independent percents. The user decides when to squeeze their total to 100%.
  def parse_allocations(raw)
    return nil if raw.blank?

    hash = raw.respond_to?(:to_unsafe_h) ? raw.to_unsafe_h : raw.to_h
    hash.transform_values { |value| value.to_s.tr(',', '.').to_f.clamp(0, 100) / 100 }
  end

  def derive_composition
    return Result::Failure.new('No exchange') if exchange.blank?

    tickers_by_asset = exchange.tickers.available.trading_enabled
                               .where(quote_asset_id:, base_asset_id: base_asset_ids)
                               .includes(:base_asset)
                               .index_by(&:base_asset_id)
    matched = allocations.filter_map do |asset_id, weight|
      ticker = tickers_by_asset[asset_id.to_i]
      next unless ticker

      {
        asset_id: asset_id.to_i,
        ticker_id: ticker.id,
        weight: weight.to_f,
        symbol: ticker.base_asset.symbol
      }
    end
    total = matched.sum { |member| member[:weight] }
    # Parked assets stay members, but may not become the whole portfolio when weighted assets vanish.
    return Result::Failure.new("None of the portfolio's weighted assets trade on #{exchange.name}") unless total.positive?

    matched.each { |member| member[:weight] = member[:weight] / total }
    Result::Success.new(matched)
  end

  def exchange_supports_current_assets?
    return false if exchange.blank?

    base_asset_ids.all? do |id|
      exchange.tickers.available.trading_enabled.exists?(base_asset_id: id, quote_asset_id:)
    end
  end

  def validate_tickers_available
    by_asset = composition_tickers.index_by(&:base_asset_id)
    valid = base_asset_ids.all? do |id|
      ticker = by_asset[id] || exchange&.tickers&.find_by(base_asset_id: id, quote_asset_id:)
      ticker.present? && ticker.available? && ticker.trading_enabled?
    end
    errors.add(:allocations, :invalid) unless valid
  end

  # Every asset that is or ever was in the composition — members plus removed holdings, which keep
  # their rows — and NOT the venue's whole quote catalogue: Alpaca decides market hours and buying
  # power from bot.tickers, and a crypto-only basket must not wait for the stock market to open
  # because an unrelated equity is quoted in the same currency. Before the first save the rows do
  # not exist yet, so the settings list stands in.
  def set_tickers
    return Ticker.none unless exchange.present?

    asset_ids = (base_asset_ids + bot_index_assets.pluck(:asset_id)).uniq
    @tickers = exchange.tickers.available.trading_enabled.where(quote_asset_id:, base_asset_id: asset_ids)
  end
end
