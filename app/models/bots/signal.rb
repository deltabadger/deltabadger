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
  # No Bot::Lifecycle: Signal is passive (no scheduling) and keeps its own thin start/stop/delete.
  include Bot::AssetConfigurable # shared asset accessors + validations (single-pair defaults)

  self.asset_id_setting_keys = %i[base_asset_id quote_asset_id]

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
      log_activity('started', details: { start_fresh: start_fresh })
      true
    else
      false
    end
  end

  def stop(stop_message_key: nil)
    if update(
      status: :stopped,
      stopped_at: Time.current,
      stop_message_key:
    )
      log_activity('stopped', details: { stop_message_key: stop_message_key }.compact)
      true
    else
      false
    end
  end

  def delete
    update(
      status: 'deleted',
      stopped_at: Time.current
    )
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

  def validate_has_signals
    return if bot_signals.any?

    errors.add(:base, I18n.t('errors.bots.signal_required'))
  end
end
