class Bots::Barbell < Bot
  include ActionCable::Channel::Broadcasting

  store_accessor :settings,
                 :base0_asset_id,
                 :base1_asset_id,
                 :quote_asset_id,
                 :quote_amount,
                 :allocation0,
                 :interval

  validates :quote_amount, presence: true, numericality: { greater_than: 0 }
  validates :interval, presence: true, inclusion: { in: INTERVALS }
  validates :allocation0, presence: true, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }
  validate :validate_barbell_bot_exchange, if: :exchange_id?, on: :update
  validate :validate_external_ids, on: :update
  validate :validate_unchangeable_assets, on: :update
  validate :validate_unchangeable_interval, on: :update

  include Schedulable
  include OrderCreator
  include Bots::Barbell::OrderSetter
  include Bots::Barbell::Measurable
  include Bots::Barbell::Fundable
  include Bots::Barbell::MarketcapAllocatable
  include SmartIntervalable # keep it up in the chain, decorators affect the interval_duration & pending_quote_amount
  include QuoteAmountLimitable
  include PriceLimitable

  def with_api_key
    exchange.set_client(api_key: api_key) if exchange.present? && (exchange.api_key.blank? || exchange.api_key != api_key)
    yield
  end

  def api_key
    @api_key ||= user.api_keys.trading.find_by(exchange_id: exchange_id) ||
                 user.api_keys.trading.new(exchange_id: exchange_id, status: :pending_validation)
  end

  def start(start_fresh: true)
    # call restarting_within_interval? before setting the status to :scheduled
    set_orders_now = start_fresh || !restarting_within_interval?
    self.status = :scheduled
    self.stop_message_key = nil
    if start_fresh
      self.started_at = Time.current
      self.last_action_job_at = nil
      self.last_successful_action_interval_checkpoint_at = nil
      self.missed_quote_amount = nil
    end

    if valid?(:start) && save
      if set_orders_now
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
    result = set_barbell_orders(
      total_orders_amount_in_quote: pending_quote_amount,
      update_missed_quote_amount: true
    )
    return result unless result.success?

    update!(status: :waiting)
    broadcast_below_minimums_warning
    Result::Success.new
  end

  def available_exchanges_for_current_settings
    base_asset_ids = [base0_asset_id, base1_asset_id].compact
    scope = ExchangeTicker.where(exchange: Exchange.available_for_barbell_bots)
    scope = scope.where(quote_asset_id: quote_asset_id) if quote_asset_id.present?
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
    available_exchanges = exchange.present? ? [exchange] : Exchange.available_for_barbell_bots
    base_asset_ids = [base0_asset_id, base1_asset_id].compact

    case asset_type
    when :base_asset
      scope = ExchangeTicker.where(exchange: available_exchanges)
                            .where.not(base_asset_id: base_asset_ids)
      scope = scope.where(quote_asset_id: quote_asset_id) if quote_asset_id.present?
    when :quote_asset
      scope = ExchangeTicker.where(exchange: available_exchanges)
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

  def restarting?
    stopped? && last_action_job_at.present?
  end

  def restarting_within_interval?
    restarting? && pending_quote_amount < quote_amount
  end

  def assets
    @assets ||= Asset.where(id: [base0_asset_id, base1_asset_id, quote_asset_id]).presence
  end

  def base0_asset
    @base0_asset ||= assets&.select { |asset| asset.id == base0_asset_id }&.first
  end

  def base1_asset
    @base1_asset ||= assets&.select { |asset| asset.id == base1_asset_id }&.first
  end

  def quote_asset
    @quote_asset ||= assets&.select { |asset| asset.id == quote_asset_id }&.first
  end

  def tickers
    @tickers ||= exchange&.tickers&.where(base_asset_id: [base0_asset_id, base1_asset_id],
                                          quote_asset_id: quote_asset_id).presence
  end

  def ticker0
    @ticker0 ||= tickers&.select { |ticker| ticker.base_asset_id == base0_asset_id }&.first
  end

  def ticker1
    @ticker1 ||= tickers&.select { |ticker| ticker.base_asset_id == base1_asset_id }&.first
  end

  def decimals
    return {} unless ticker0.present? && ticker1.present?

    @decimals ||= {
      base0: ticker0.base_decimals,
      base1: ticker1.base_decimals,
      quote: [ticker0.quote_decimals, ticker1.quote_decimals].max,
      base0_price: ticker0.price_decimals,
      base1_price: ticker1.price_decimals
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
      partial: 'bots/barbell/warning_below_minimums',
      locals: locals_for_below_minimums_warning(first_transaction, second_transaction)
    )
  end

  private

  def validate_external_ids
    errors.add(:base0_asset_id, :invalid) unless Asset.exists?(base0_asset_id)
    errors.add(:base1_asset_id, :invalid) unless Asset.exists?(base1_asset_id)
    errors.add(:quote_asset_id, :invalid) unless Asset.exists?(quote_asset_id)
  end

  def validate_barbell_bot_exchange
    return if exchange.tickers.exists?(base_asset: base0_asset, quote_asset: quote_asset) &&
              exchange.tickers.exists?(base_asset: base1_asset, quote_asset: quote_asset)

    errors.add(:exchange, :unsupported, message: I18n.t('errors.bots.exchange_asset_mismatch', exchange_name: exchange.name))
  end

  def validate_unchangeable_assets
    return unless transactions.exists?
    return unless settings_changed?

    errors.add(:base0_asset_id, :unchangeable) if settings_was['base0_asset_id'] != settings['base0_asset_id']
    errors.add(:base1_asset_id, :unchangeable) if settings_was['base1_asset_id'] != settings['base1_asset_id']
    errors.add(:quote_asset_id, :unchangeable) if settings_was['quote_asset_id'] != settings['quote_asset_id']
  end

  def validate_unchangeable_interval
    return unless working?
    return unless settings_changed?
    return unless settings_was['interval'] != settings['interval']

    errors.add(:settings, :unchangeable_interval,
               message: 'Interval cannot be changed while the bot is running')
  end

  def action_job_config
    {
      queue: exchange.name_id,
      class: 'Bot::ActionJob',
      args: [{ '_aj_globalid' => to_global_id.to_s }]
    }
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
