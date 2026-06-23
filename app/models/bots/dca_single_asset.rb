class Bots::DcaSingleAsset < Bot
  include ActionCable::Channel::Broadcasting

  store_accessor :settings,
                 :base_asset_id,
                 :quote_asset_id,
                 :quote_amount,
                 :interval

  validates :quote_amount, presence: true, numericality: { greater_than: 0 }
  validate :validate_bot_exchange, if: :exchange_id?, on: :update
  validate :validate_external_ids, on: :update
  validate :validate_unchangeable_assets, on: :update
  validate :validate_unchangeable_interval, on: :update
  validate :validate_unchangeable_exchange, on: :update
  validate :validate_tickers_available, on: :start

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
  include Bots::DcaSingleAsset::OrderSetter
  include Bots::DcaSingleAsset::Measurable
  include Bot::Lifecycle         # shared start/stop/delete — keep LAST so the stop decorators above stay on top
  include Bot::AssetConfigurable # shared asset accessors + validations (single-pair defaults)
  include Bot::LimitCheckable    # live limit-check job from limit_paused log (recovery/rescue)

  self.asset_id_setting_keys = %i[base_asset_id quote_asset_id]

  def parse_params(params)
    {
      base_asset_id: params[:base_asset_id].presence&.to_i,
      quote_asset_id: params[:quote_asset_id].presence&.to_i,
      quote_amount: params[:quote_amount].presence&.to_f,
      interval: params[:interval].presence
    }.compact
  end

  def execute_action
    with_api_key do
      update!(status: :executing)
      result = set_order(
        order_amount_in_quote: pending_quote_amount,
        update_missed_quote_amount: true
      )
      return result if result.failure?

      update!(status: :waiting)
      broadcast_below_minimums_warning
      Result::Success.new
    end
  end

  def base_asset
    @base_asset ||= asset_with_id(base_asset_id)
  end

  def quote_asset
    @quote_asset ||= asset_with_id(quote_asset_id)
  end

  def ticker
    @ticker ||= tickers.first
  end

  def tickers_for_start
    [ticker]
  end

  def decimals
    return {} unless ticker.present?

    @decimals ||= {
      base: ticker&.base_decimals,
      quote: ticker&.quote_decimals,
      base_price: ticker&.price_decimals
    }
  end

  def broadcast_below_minimums_warning
    first_transactions = transactions.limit(2)
    return unless first_transactions.count == 1
    return unless first_transactions.first.skipped?

    first_transaction = first_transactions.first

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'modal',
      partial: 'bots/dca_single_assets/warning_below_minimums',
      locals: locals_for_below_minimums_warning(first_transaction)
    )
  end

  private

  def locals_for_below_minimums_warning(first_transaction)
    ticker = first_transaction.exchange.tickers.find_by(
      base_asset_id: first_transaction.base_asset.id,
      quote_asset_id: first_transaction.quote_asset.id
    )
    {
      quote_symbol: first_transaction.quote_asset.symbol,
      missed_symbol: first_transaction.base_asset.symbol,
      missed_minimum_base_size: ticker.minimum_base_size,
      missed_minimum_quote_size: ticker.minimum_quote_size,
      exchange_name: first_transaction.exchange.name
    }
  end
end
