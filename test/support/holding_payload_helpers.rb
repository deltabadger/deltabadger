# A composition bot's metrics payload names the asset behind each holding key (`key_assets`), which is
# what every reader matches by. Tests that stub a payload by symbol key it here the way the walk would:
# each key is the asset with that symbol.
module HoldingPayloadHelpers
  def keyed_payload(payload)
    return payload unless payload.is_a?(Hash)

    keys = payload.values_at(:asset_values, :asset_breakdown, :asset_lots).compact.flat_map(&:keys).uniq
    derived = keys.to_h { |key| [key, Asset.find_by(symbol: key)&.id] }
    payload.merge(key_assets: derived.merge(payload[:key_assets] || {}) { |_key, mine, theirs| theirs || mine })
  end

  def asset_id_of(symbol) = Asset.find_by!(symbol:).id
  def asset_ids_of(symbols) = symbols.map { |symbol| asset_id_of(symbol) }

  # [[key, asset_id], ...] for a liquidation naming these holdings.
  def holdings_named(keys) = Array(keys).map { |key| [key, Asset.find_by(symbol: key)&.id] }
end

ActiveSupport::TestCase.include HoldingPayloadHelpers
