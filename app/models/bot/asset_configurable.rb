# Shared asset/ticker plumbing and settings validations for all bot types.
#
# Each bot class declares which of its settings keys hold Asset ids:
#
#   self.asset_id_setting_keys = %i[base_asset_id quote_asset_id]
#
# The declaration drives #assets, validate_external_ids and
# validate_unchangeable_assets. Validation REGISTRATION (`validate :...`)
# stays in each bot class — types differ in which validations they run and
# in which contexts.
#
# The market-scope and ticker methods default to the single-pair shape
# (base_asset_id/quote_asset_id), shared by DcaSingleAsset and Signal;
# DcaDualAsset and DcaIndex override them with their genuinely different
# queries.
#
# Include LAST in the bot model so dispatch matches the former per-class
# definitions (above all other included concerns, below the prepended
# decorator chains and the class's own overrides).
module Bot::AssetConfigurable
  extend ActiveSupport::Concern

  included do
    class_attribute :asset_id_setting_keys, default: [].freeze, instance_writer: false
  end

  def assets
    @assets ||= Asset.where(id: asset_id_setting_keys.map { |key| public_send(key) })
  end

  def tickers
    @tickers ||= set_tickers
  end

  def available_exchanges_for_current_settings
    scope = Ticker.available.trading_enabled.where(exchange: Exchange.available)
    scope = scope.where(quote_asset_id:) if quote_asset_id.present?
    scope = scope.where(base_asset_id:) if base_asset_id.present?
    exchange_ids = scope.pluck(:exchange_id).uniq
    Exchange.where(id: exchange_ids)
  end

  # @param asset_type: :base_asset or :quote_asset
  def available_assets_for_current_settings(asset_type:, include_exchanges: false)
    available_exchanges = exchange.present? ? [exchange] : Exchange.available

    case asset_type
    when :base_asset
      scope = Ticker.available.trading_enabled
                    .where(exchange: available_exchanges)
                    .where.not(base_asset_id: [base_asset_id, quote_asset_id])
      scope = scope.where(quote_asset_id:) if quote_asset_id.present?
    when :quote_asset
      scope = Ticker.available.trading_enabled
                    .where(exchange: available_exchanges)
                    .where.not(quote_asset_id: [base_asset_id, quote_asset_id])
      scope = scope.where(base_asset_id:) if base_asset_id.present?
    end
    asset_ids = scope.pluck("#{asset_type}_id").uniq
    include_exchanges ? Asset.includes(:exchanges).where(id: asset_ids) : Asset.where(id: asset_ids)
  end

  private

  def asset_with_id(id)
    assets.detect { |asset| asset.id == id }
  end

  def set_tickers
    @tickers = exchange&.tickers&.where(base_asset_id:, quote_asset_id:) ||
               Ticker.none
  end

  def validate_external_ids
    asset_id_setting_keys.each do |key|
      errors.add(key, :invalid) unless Asset.exists?(public_send(key))
    end
  end

  def validate_unchangeable_assets
    return unless settings_changed?
    return unless transactions.any?

    asset_id_setting_keys.each do |key|
      errors.add(key, :unchangeable) if public_send("#{key}_was") != public_send(key)
    end
  end

  def validate_unchangeable_interval
    return unless settings_changed?
    return unless working?
    return unless interval_was != interval

    errors.add(:settings, :unchangeable_interval,
               message: 'Interval cannot be changed while the bot is running')
  end

  def validate_unchangeable_exchange
    return unless exchange_id_changed?
    return unless transactions.waiting.any?

    errors.add(:exchange, :unchangeable,
               message: I18n.t('errors.bots.exchange_change_while_open_orders', exchange_name: exchange.name))
  end

  def validate_bot_exchange
    return if stopped? || deleted?
    return if exchange_supports_current_assets?

    errors.add(:exchange, :unsupported, message: I18n.t('errors.bots.exchange_asset_mismatch', exchange_name: exchange.name))
  end

  # Default: the single-pair existence check. Multi-asset types override with
  # their own queries (dual: both pairs; index: any pair on the quote).
  def exchange_supports_current_assets?
    exchange.tickers.available.trading_enabled.exists?(base_asset:, quote_asset:)
  end

  def validate_tickers_available
    return if ticker.present? && ticker.available? && ticker.trading_enabled?

    errors.add(:base_asset_id, :invalid)
    errors.add(:quote_asset_id, :invalid)
  end
end
