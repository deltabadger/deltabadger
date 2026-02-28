class Bots::Signal < Bot
  include ActionCable::Channel::Broadcasting

  store_accessor :settings,
                 :base_asset_id,
                 :quote_asset_id

  validate :validate_bot_exchange, if: :exchange_id?, on: :update
  validate :validate_external_ids, on: :update
  validate :validate_unchangeable_assets, on: :update
  validate :validate_unchangeable_exchange, on: :update
  validate :validate_tickers_available, on: :start
  validate :validate_has_signals, on: :start

  before_save :set_tickers, if: :will_save_change_to_exchange_id?

  has_many :bot_signals, foreign_key: :bot_id, dependent: :destroy, inverse_of: :bot

  include Exportable
  include Bots::DcaSingleAsset::Measurable

  def api_key_type
    :trading
  end

  def parse_params(params)
    {
      base_asset_id: params[:base_asset_id].presence&.to_i,
      quote_asset_id: params[:quote_asset_id].presence&.to_i
    }.compact
  end

  def start(start_fresh: true)
    self.status = :scheduled
    self.stop_message_key = nil
    self.started_at = Time.current if start_fresh

    if valid?(:start) && save
      true
    else
      false
    end
  end

  def stop(stop_message_key: nil)
    update(
      status: :stopped,
      stopped_at: Time.current,
      stop_message_key:
    )
  end

  def delete
    update(
      status: 'deleted',
      stopped_at: Time.current
    )
  end

  def available_exchanges_for_current_settings
    scope = Ticker.available.where(exchange: Exchange.available)
    scope = scope.where(quote_asset_id:) if quote_asset_id.present?
    scope = scope.where(base_asset_id:) if base_asset_id.present?
    exchange_ids = scope.pluck(:exchange_id).uniq
    Exchange.where(id: exchange_ids)
  end

  def available_assets_for_current_settings(asset_type:, include_exchanges: false)
    available_exchanges = exchange.present? ? [exchange] : Exchange.available

    case asset_type
    when :base_asset
      scope = Ticker.available
                    .where(exchange: available_exchanges)
                    .where.not(base_asset_id: [base_asset_id, quote_asset_id])
      scope = scope.where(quote_asset_id:) if quote_asset_id.present?
    when :quote_asset
      scope = Ticker.available
                    .where(exchange: available_exchanges)
                    .where.not(quote_asset_id: [base_asset_id, quote_asset_id])
      scope = scope.where(base_asset_id:) if base_asset_id.present?
    end
    asset_ids = scope.pluck("#{asset_type}_id").uniq
    include_exchanges ? Asset.includes(:exchanges).where(id: asset_ids) : Asset.where(id: asset_ids)
  end

  def assets
    @assets ||= Asset.where(id: [base_asset_id, quote_asset_id])
  end

  def base_asset
    @base_asset ||= assets.select { |asset| asset.id == base_asset_id }.first
  end

  def quote_asset
    @quote_asset ||= assets.select { |asset| asset.id == quote_asset_id }.first
  end

  def tickers
    @tickers ||= set_tickers
  end

  def ticker
    @ticker ||= tickers.first
  end

  def decimals
    return {} unless ticker.present?

    @decimals ||= {
      base: ticker&.base_decimals,
      quote: ticker&.quote_decimals,
      base_price: ticker&.price_decimals
    }
  end

  # Passive bot — no interval-based scheduling
  def progress_percentage
    0
  end

  def last_action_job_at
    nil
  end

  def next_action_job_at
    nil
  end

  def restarting?
    false
  end

  private

  def validate_external_ids
    errors.add(:base_asset_id, :invalid) unless Asset.exists?(base_asset_id)
    errors.add(:quote_asset_id, :invalid) unless Asset.exists?(quote_asset_id)
  end

  def validate_bot_exchange
    return if exchange.tickers.available.exists?(base_asset:, quote_asset:)

    errors.add(:exchange, :unsupported, message: I18n.t('errors.bots.exchange_asset_mismatch', exchange_name: exchange.name))
  end

  def validate_unchangeable_assets
    return unless settings_changed?
    return unless transactions.any?

    errors.add(:base_asset_id, :unchangeable) if base_asset_id_was != base_asset_id
    errors.add(:quote_asset_id, :unchangeable) if quote_asset_id_was != quote_asset_id
  end

  def validate_unchangeable_exchange
    return unless exchange_id_changed?
    return unless transactions.open.any?

    errors.add(:exchange, :unchangeable,
               message: I18n.t('errors.bots.exchange_change_while_open_orders', exchange_name: exchange.name))
  end

  def validate_tickers_available
    return if ticker.present? && ticker.available?

    errors.add(:base_asset_id, :invalid)
    errors.add(:quote_asset_id, :invalid)
  end

  def validate_has_signals
    return if bot_signals.any?

    errors.add(:base, I18n.t('errors.bots.signal_required'))
  end

  def set_tickers
    @tickers = exchange&.tickers&.where(base_asset_id:, quote_asset_id:) ||
               Ticker.none
  end
end
