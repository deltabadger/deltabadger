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
  validate :validate_dca_single_asset_included_in_subscription_plan, on: :start

  before_save :set_tickers, if: :will_save_change_to_exchange_id?
  # TODO: If bots can change assets, we also need to update the tickers and assets values
  #       ! also in price_limitable

  include SmartIntervalable      # decorators for: parse_params, pending_quote_amount, interval_duration, restarting_within_interval?
  include LimitOrderable         # decorators for: parse_params
  include QuoteAmountLimitable   # decorators for: parse_params, pending_quote_amount
  include PriceLimitable         # decorators for: parse_params, started_at, execute_action, stop
  include PriceDropLimitable     # decorators for: parse_params, started_at, execute_action, stop
  include MovingAverageLimitable # decorators for: parse_params, started_at, execute_action, stop
  include IndicatorLimitable     # decorators for: parse_params, started_at, execute_action, stop
  include Fundable
  include Schedulable
  include OrderCreator
  include Accountable
  include Bots::DcaSingleAsset::OrderSetter
  include Bots::DcaSingleAsset::Measurable

  def with_api_key
    exchange.set_client(api_key: api_key) if exchange.present? && (exchange.api_key.blank? || exchange.api_key != api_key)
    yield
  end

  def api_key
    @api_key ||= user.api_keys.trading.find_by(exchange_id: exchange_id) ||
                 user.api_keys.trading.new(exchange_id: exchange_id, status: :pending_validation)
  end

  def parse_params(params)
    {
      base_asset_id: params[:base_asset_id].presence&.to_i,
      quote_asset_id: params[:quote_asset_id].presence&.to_i,
      quote_amount: params[:quote_amount].presence&.to_f,
      interval: params[:interval].presence
    }.compact
  end

  def start(start_fresh: true)
    # call restarting_within_interval? before setting the status to :scheduled
    set_order_now = start_fresh || !restarting_within_interval?
    self.status = :scheduled
    self.stop_message_key = nil
    if start_fresh
      self.started_at = Time.current
      self.last_action_job_at = nil
      self.missed_quote_amount = nil
    end

    if valid?(:start) && save
      if set_order_now
        Bot::ActionJob.perform_later(self)
      else
        Bot::ActionJob.set(wait_until: next_interval_checkpoint_at).perform_later(self)
        Bot::BroadcastAfterScheduledActionJob.perform_later(self)
      end
      true
    else
      false
    end
  end

  def stop(stop_message_key: nil)
    if update(
      status: :stopped,
      stopped_at: Time.current,
      stop_message_key: stop_message_key
    )
      cancel_scheduled_action_jobs
      true
    else
      false
    end
  end

  def delete
    if update(
      status: 'deleted',
      stopped_at: Time.current
    )
      cancel_scheduled_action_jobs if exchange.present?
      true
    else
      false
    end
  end

  def execute_action
    notify_if_funds_are_low
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

  def available_exchanges_for_current_settings
    scope = ExchangeTicker.where(exchange: Exchange.available_for_new_bots)
    scope = scope.where(quote_asset_id: quote_asset_id) if quote_asset_id.present?
    scope = scope.where(base_asset_id: base_asset_id) if base_asset_id.present?
    exchange_ids = scope.pluck(:exchange_id).uniq
    Exchange.where(id: exchange_ids)
  end

  # @param asset_type: :base_asset or :quote_asset
  def available_assets_for_current_settings(asset_type:, include_exchanges: false)
    available_exchanges = exchange.present? ? [exchange] : Exchange.available_for_new_bots

    case asset_type
    when :base_asset
      scope = ExchangeTicker.where(exchange: available_exchanges)
                            .where.not(base_asset_id: [base_asset_id, quote_asset_id])
      scope = scope.where(quote_asset_id: quote_asset_id) if quote_asset_id.present?
    when :quote_asset
      scope = ExchangeTicker.where(exchange: available_exchanges)
                            .where.not(quote_asset_id: [base_asset_id, quote_asset_id])
      scope = scope.where(base_asset_id: base_asset_id) if base_asset_id.present?
    end
    asset_ids = scope.pluck("#{asset_type}_id").uniq
    include_exchanges ? Asset.includes(:exchanges).where(id: asset_ids) : Asset.where(id: asset_ids)
  end

  def restarting?
    stopped? && last_action_job_at.present?
  end

  def restarting_within_interval?
    restarting? && pending_quote_amount < quote_amount
  end

  def assets
    @assets ||= Asset.where(id: [base_asset_id, quote_asset_id]).presence
  end

  def base_asset
    @base_asset ||= assets&.select { |asset| asset.id == base_asset_id }&.first
  end

  def quote_asset
    @quote_asset ||= assets&.select { |asset| asset.id == quote_asset_id }&.first
  end

  def tickers
    @tickers ||= set_tickers
  end

  def ticker
    @ticker ||= tickers&.first
  end

  def decimals
    return {} unless ticker.present?

    @decimals ||= {
      base: ticker.base_decimals,
      quote: ticker.quote_decimals,
      base_price: ticker.price_decimals
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

  def validate_external_ids
    errors.add(:base_asset_id, :invalid) unless Asset.exists?(base_asset_id)
    errors.add(:quote_asset_id, :invalid) unless Asset.exists?(quote_asset_id)
  end

  def validate_bot_exchange
    return if exchange.tickers.exists?(base_asset: base_asset, quote_asset: quote_asset)

    errors.add(:exchange, :unsupported, message: I18n.t('errors.bots.exchange_asset_mismatch', exchange_name: exchange.name))
  end

  def validate_unchangeable_assets
    return unless transactions.exists?
    return unless settings_changed?

    errors.add(:base_asset_id, :unchangeable) if base_asset_id_was != base_asset_id
    errors.add(:quote_asset_id, :unchangeable) if quote_asset_id_was != quote_asset_id
  end

  def validate_unchangeable_interval
    return unless working?
    return unless settings_changed?
    return unless interval_was != interval

    errors.add(:settings, :unchangeable_interval,
               message: 'Interval cannot be changed while the bot is running')
  end

  def validate_dca_single_asset_included_in_subscription_plan; end

  def set_tickers
    @tickers = exchange&.tickers&.where(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id).presence
  end

  def action_job_config
    {
      queue: exchange.name_id,
      class: 'Bot::ActionJob',
      args: [{ '_aj_globalid' => to_global_id.to_s }]
    }
  end

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
