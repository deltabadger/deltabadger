class Bots::DcaDualAsset < Bot
  include ActionCable::Channel::Broadcasting

  store_accessor :settings,
                 :base0_asset_id,
                 :base1_asset_id,
                 :quote_asset_id,
                 :quote_amount,
                 :allocation0,
                 :interval

  validates :quote_amount, presence: true, numericality: { greater_than: 0 }
  validates :allocation0, presence: true, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }
  validate :validate_bot_exchange, if: :exchange_id?, on: :update
  validate :validate_external_ids, on: :update
  validate :validate_unchangeable_assets, on: :update
  validate :validate_unchangeable_interval, on: :update
  validate :validate_unchangeable_exchange, on: :update
  validate :validate_tickers_available, on: :start

  after_initialize :initialize_dca_dual_asset_settings
  before_save :set_tickers, if: :will_save_change_to_exchange_id?

  include SmartIntervalable      # decorators for: parse_params, effective_quote_amount, effective_interval_duration
  include LimitOrderable         # decorators for: parse_params, execute_action
  include QuoteAmountLimitable   # decorators for: parse_params, pending_quote_amount
  include PriceLimitable         # decorators for: parse_params, started_at, execute_action, stop
  include PriceDropLimitable     # decorators for: parse_params, started_at, execute_action, stop
  include MovingAverageLimitable # decorators for: parse_params, started_at, execute_action, stop
  include IndicatorLimitable     # decorators for: parse_params, started_at, execute_action, stop
  include Fundable               # decorators for: execute_action
  include Automation::Schedulable
  include Bot::Startable         # decorators for: parse_params; overrides Schedulable defaults — keep AFTER Schedulable
  include OrderCreator
  include Accountable
  include Exportable
  include Bots::DcaDualAsset::MarketcapAllocatable # decorators for: parse_params
  include Bots::DcaDualAsset::OrderSetter
  include Bots::DcaDualAsset::Measurable
  include Bot::Lifecycle         # shared start/stop/delete — keep LAST so the stop decorators above stay on top
  include Bot::AssetConfigurable # shared asset accessors + validations (the available_*/ticker queries below override the single-pair defaults)

  self.asset_id_setting_keys = %i[base0_asset_id base1_asset_id quote_asset_id]

  def parse_params(params)
    {
      base0_asset_id: params[:base0_asset_id].presence&.to_i,
      base1_asset_id: params[:base1_asset_id].presence&.to_i,
      quote_asset_id: params[:quote_asset_id].presence&.to_i,
      quote_amount: params[:quote_amount].presence&.to_f,
      interval: params[:interval].presence,
      allocation0: params[:allocation0].presence&.to_f
    }.compact
  end

  def execute_action
    update!(status: :executing)
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
    base_asset_ids = [base0_asset_id, base1_asset_id].compact
    scope = Ticker.available.trading_enabled.where(exchange: Exchange.available)
    scope = scope.where(quote_asset_id:) if quote_asset_id.present?
    scope = scope.where(base_asset_id: base_asset_ids) if base_asset_ids.any?
    exchange_ids = if base_asset_ids.size > 1
                     scope.group_by(&:exchange_id)
                          .transform_values { |tickers| tickers.map(&:base_asset_id).uniq }
                          .select { |_, b_a_ids| b_a_ids.size >= base_asset_ids.size }
                          .keys
                   else
                     scope.pluck(:exchange_id).uniq
                   end
    Exchange.where(id: exchange_ids)
  end

  # @param asset_type: :base_asset or :quote_asset
  def available_assets_for_current_settings(asset_type:, include_exchanges: false)
    available_exchanges = exchange.present? ? [exchange] : Exchange.available
    base_asset_ids = [base0_asset_id, base1_asset_id].compact

    case asset_type
    when :base_asset
      # When picking second asset, narrow to exchanges that also have the first asset
      if exchange.blank? && base_asset_ids.any?
        shared_exchange_ids = Ticker.available.trading_enabled.where(exchange: available_exchanges, base_asset_id: base_asset_ids)
                                    .pluck(:exchange_id).uniq
        available_exchanges = Exchange.where(id: shared_exchange_ids)
      end
      scope = Ticker.available.trading_enabled
                    .where(exchange: available_exchanges)
                    .where.not(base_asset_id: base_asset_ids + [quote_asset_id])
      scope = scope.where(quote_asset_id:) if quote_asset_id.present?
    when :quote_asset
      scope = Ticker.available.trading_enabled
                    .where(exchange: available_exchanges)
                    .where.not(quote_asset_id: base_asset_ids + [quote_asset_id])
      if base_asset_ids.any?
        scope = scope.where(base_asset_id: base_asset_ids)
        valid_quote_asset_ids = scope.pluck(:quote_asset_id, :base_asset_id)
                                     .group_by(&:first)
                                     .transform_values { |pairs| pairs.map(&:last) }
                                     .select { |_, bb| base_asset_ids.map { |b| bb.include?(b.to_i) }.all? }
                                     .keys
        scope = scope.where(quote_asset_id: valid_quote_asset_ids)
      end
    end
    asset_ids = scope.pluck("#{asset_type}_id").uniq
    include_exchanges ? Asset.includes(:exchanges).where(id: asset_ids) : Asset.where(id: asset_ids)
  end

  def base0_asset
    @base0_asset ||= asset_with_id(base0_asset_id)
  end

  def base1_asset
    @base1_asset ||= asset_with_id(base1_asset_id)
  end

  def quote_asset
    @quote_asset ||= asset_with_id(quote_asset_id)
  end

  def ticker0
    @ticker0 ||= tickers.select { |ticker| ticker.base_asset_id == base0_asset_id }.first
  end

  def ticker1
    @ticker1 ||= tickers.select { |ticker| ticker.base_asset_id == base1_asset_id }.first
  end

  def tickers_for_start
    [ticker0, ticker1]
  end

  def decimals
    return {} unless ticker0.present? && ticker1.present?

    @decimals ||= {
      base0: ticker0&.base_decimals,
      base1: ticker1&.base_decimals,
      quote: [ticker0&.quote_decimals, ticker1&.quote_decimals].compact.max,
      base0_price: ticker0&.price_decimals,
      base1_price: ticker1&.price_decimals
    }
  end

  def broadcast_below_minimums_warning
    first_transactions = transactions.limit(3)
    return unless first_transactions.count == 2
    return unless [first_transactions.first.skipped?, first_transactions.last.skipped?].any?

    first_transaction = first_transactions.first
    second_transaction = first_transactions.last

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'modal',
      partial: 'bots/dca_dual_assets/warning_below_minimums',
      locals: locals_for_below_minimums_warning(first_transaction, second_transaction)
    )
  end

  private

  def exchange_supports_current_assets?
    exchange.tickers.available.trading_enabled.exists?(base_asset: base0_asset, quote_asset:) &&
      exchange.tickers.available.trading_enabled.exists?(base_asset: base1_asset, quote_asset:)
  end

  def validate_tickers_available
    return if ticker0.present? && ticker0.available? && ticker0.trading_enabled? &&
              ticker1.present? && ticker1.available? && ticker1.trading_enabled?

    errors.add(:base0_asset_id, :invalid) unless ticker0.present? && ticker0.available? && ticker0.trading_enabled?
    errors.add(:base1_asset_id, :invalid) unless ticker1.present? && ticker1.available? && ticker1.trading_enabled?
    errors.add(:quote_asset_id, :invalid)
  end

  def initialize_dca_dual_asset_settings
    self.allocation0 ||= 0.5
  end

  def set_tickers
    @tickers = exchange&.tickers&.where(base_asset_id: [base0_asset_id, base1_asset_id],
                                        quote_asset_id:) ||
               Ticker.none
  end

  def locals_for_below_minimums_warning(first_transaction, second_transaction)
    if first_transaction.skipped? && second_transaction.skipped?
      ticker0 = first_transaction.exchange.tickers.find_by(
        base_asset_id: first_transaction.base_asset.id,
        quote_asset_id: first_transaction.quote_asset.id
      )
      ticker1 = second_transaction.exchange.tickers.find_by(
        base_asset_id: second_transaction.base_asset.id,
        quote_asset_id: second_transaction.quote_asset.id
      )
      {
        base0_symbol: first_transaction.base_asset.symbol,
        base1_symbol: second_transaction.base_asset.symbol,
        base0_minimum_base_size: ticker0.minimum_base_size,
        base0_minimum_quote_size: ticker0.minimum_quote_size,
        quote_symbol: first_transaction.quote_asset.symbol,
        base1_minimum_base_size: ticker1.minimum_base_size,
        base1_minimum_quote_size: ticker1.minimum_quote_size,
        exchange_name: first_transaction.exchange.name,
        missed_count: 2
      }
    else
      bought_transaction = first_transaction.skipped? ? second_transaction : first_transaction
      missed_transaction = first_transaction.skipped? ? first_transaction : second_transaction
      {
        bought_quote_amount: bought_transaction.quote_amount,
        quote_symbol: bought_transaction.quote_asset.symbol,
        bought_symbol: bought_transaction.base_asset.symbol,
        missed_symbol: missed_transaction.base_asset.symbol,
        missed_minimum_base_size: missed_transaction.base_asset.min_base_size,
        missed_minimum_quote_size: missed_transaction.base_asset.min_quote_size,
        exchange_name: first_transaction.exchange.name,
        missed_count: 1
      }
    end
  end
end
