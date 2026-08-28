module Bots::DcaMultiAsset::Allocatable
  extend ActiveSupport::Concern

  # Sliders write raw fractional weights; normalize_allocations squeezes them only when requested.
  ALLOCATION_TOLERANCE = 0.001

  included do
    before_validation :derive_allocations_from_base_asset_ids, on: :create
    validate :validate_allocations
    validate :validate_allocations_balanced, on: :start
    validate :validate_composition_pairs,
             if: -> { exchange_id? && (new_record? || composition_changed_pending? || will_save_change_to_exchange_id?) }
    validate :validate_composition_frozen_while_working, on: :update
  end

  def allocations
    value = super
    value.is_a?(Hash) ? value : {}
  end

  # Wizard builds carry the raw list and no weights yet; everything else reads the weights' keys.
  def base_asset_ids
    return allocations.keys.map(&:to_i) if allocations.any?

    Array(settings['base_asset_ids']).map(&:to_i)
  end

  def base_assets
    by_id = Asset.where(id: base_asset_ids).index_by(&:id)
    base_asset_ids.filter_map { |id| by_id[id] }
  end

  def allocation_for(asset_id) = allocations[asset_id.to_s].to_f

  def allocations_total = allocations.values.sum(&:to_f)

  def allocations_balanced?
    # A derived basket does not own its weights, so the slider total is not a gate on starting it.
    return true if try(:market_cap_weighted?)

    (allocations_total - 1).abs <= ALLOCATION_TOLERANCE
  end

  # What the settings page must render. For a manual basket that is the slider the user set; for a
  # derived one it is the weight actually being traded. Reading allocation_for unconditionally would
  # show a converted 70/30 basket as 70/30 while it traded 90/10.
  def displayed_allocation_for(asset_id)
    return allocation_for(asset_id) unless try(:market_cap_weighted?)

    bot_index_assets.in_index.find_by(asset_id:)&.target_allocation&.to_f || allocation_for(asset_id)
  end

  # The header total has to come from the same source as the rows, or a derived basket prints the
  # slider sum above rows that add up to 100%.
  def displayed_allocations_total
    return allocations_total unless try(:market_cap_weighted?)

    base_asset_ids.sum { |id| displayed_allocation_for(id) }
  end

  def composition_locked? = working? || rebalance_pending?

  # String keys (JSON), floats, 3 dp, sum exactly 1. Three decimals is the slider's 0.1% step: a
  # finer weight would be snapped by the range input on the next render and stop adding up to 100%.
  # Negative inputs clamp to 0. The residual goes to the largest weight: on the last key it could
  # make a tiny allocation negative. Zero is a legal weight — a parked asset stays a member.
  def normalize_allocations(hash)
    entries = hash.to_h { |key, value| [key.to_s, [value.to_f, 0].max] }
    total = entries.values.sum
    return entries if entries.empty?

    entries.transform_values! { 1.0 } unless total.positive?
    total = entries.values.sum

    rounded = entries.transform_values { |value| (value / total).round(3) }
    largest = rounded.max_by { |_, value| value }.first
    rounded[largest] = (rounded[largest] + (1 - rounded.values.sum)).round(3)
    rounded
  end

  def equal_allocations(ids) = normalize_allocations(ids.uniq.to_h { |id| [id, 1.0] })

  def allocations_adding(asset_id, base = allocations)
    return base if base.key?(asset_id.to_s)

    base.merge(asset_id.to_s => 0.0)
  end

  def allocations_removing(asset_id, base = allocations)
    base.except(asset_id.to_s)
  end

  # Settings are compared key-by-key: store_accessor rewrites the column on every save.
  def composition_changed?
    before, after = saved_changes['settings']
    before.present? && self.class::COMPOSITION_KEYS.any? { |key| before[key] != after[key] }
  end

  def composition_changed_pending?
    return false unless will_save_change_to_settings?

    before = settings_was || {}
    self.class::COMPOSITION_KEYS.any? { |key| before[key] != settings[key] }
  end

  private

  def derive_allocations_from_base_asset_ids
    ids = Array(settings['base_asset_ids']).map(&:to_i).reject(&:zero?)
    self.allocations = equal_allocations(ids) if allocations.empty? && ids.any?
    settings.delete('base_asset_ids')
  end

  def validate_allocations
    keys = allocations.keys
    min = self.class::MIN_ASSETS
    max = self.class::MAX_ASSETS
    if keys.size < min
      errors.add(:allocations, :too_few,
                 message: I18n.t('errors.bots.multi_asset.min_assets', min:))
    end
    if keys.size > max
      errors.add(:allocations, :too_many,
                 message: I18n.t('errors.bots.multi_asset.max_assets', max:))
    end

    # A malformed value is a validation error, not an exception from #sum or #between?.
    values = allocations.values
    numeric = values.all? { |value| value.is_a?(Numeric) && (!value.respond_to?(:finite?) || value.finite?) }
    if numeric
      errors.add(:allocations, :invalid) unless values.all? { |value| value.between?(0, 1) }
    else
      errors.add(:allocations, :invalid)
    end
    errors.add(:allocations, :invalid) if quote_asset_id.present? && keys.include?(quote_asset_id.to_s)
    errors.add(:allocations, :invalid) unless Asset.where(id: keys).count == keys.size
  end

  def validate_allocations_balanced
    return if allocations_balanced?

    errors.add(:allocations, :unbalanced, message: I18n.t('bot.dca_multi_asset.normalize_first'))
  end

  # This is the authoritative pair check. A stopped bot may not be given an asset its venue does
  # not trade either, so the status-based generic exchange validation is deliberately not used.
  def validate_composition_pairs
    # A crafted id with no row behind it must be a validation error, not a NoMethodError.
    return errors.add(:exchange, :invalid) if exchange.blank?

    missing = base_asset_ids.reject do |id|
      exchange.tickers.available.trading_enabled.exists?(base_asset_id: id, quote_asset_id:)
    end
    return if missing.empty?

    symbols = Asset.where(id: missing).pluck(:symbol).to_sentence
    errors.add(
      :allocations,
      :pair_missing,
      message: I18n.t('errors.bots.multi_asset.pair_missing', assets: symbols, exchange_name: exchange.name)
    )
  end

  # The UI and server share one lock boundary so a pending swap cannot change the portfolio it sold.
  def validate_composition_frozen_while_working
    # A pending rebalance pins the venue even on a stopped bot: its sell leg happened on this
    # exchange and its buy leg is still owed here. validate_unchangeable_exchange only looks for
    # waiting orders, which a transient buy rejection leaves none of.
    if will_save_change_to_exchange_id? && rebalance_pending?
      errors.add(:exchange, :locked, message: I18n.t('errors.bots.multi_asset.locked_while_running'))
    end
    return unless composition_locked? && (composition_changed_pending? || will_save_change_to_exchange_id?)

    errors.add(
      :allocations,
      :locked,
      message: I18n.t('errors.bots.multi_asset.locked_while_running')
    )
  end
end
