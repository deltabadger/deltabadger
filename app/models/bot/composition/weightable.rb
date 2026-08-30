# The weight formula shared by every composition bot that derives its own allocations.
#
# Pure arithmetic on numbers the caller has already gathered — no I/O, deliberately. A composition
# bot reconciles its membership synchronously after save (dca_multi_asset.rb:19), so acquiring the
# market caps is the caller's business and must not become a network call inside derive_composition.
#
# flattening interpolates between the two ends: 0 = pure market cap, 1 = equal weight. The index bot
# exposes it as a user setting; the multi-asset bot uses the pure-market-cap end, which is what the
# retired pair bot's market-cap switch meant.
module Bot::Composition::Weightable
  module_function

  # @param market_caps [Hash] { asset_id => Numeric }. The caller decides what to do about missing
  #   or zero data before calling — see Bots::DcaMultiAsset#derive_composition.
  # @param flattening [Numeric] 0..1
  # @return [Hash] { asset_id => Float }, summing to 1.0 (empty in, empty out)
  def blend(market_caps:, flattening:)
    return {} if market_caps.empty?

    equal_weight = 1.0 / market_caps.size
    total = market_caps.values.sum(&:to_f)
    flattening = flattening.to_f.clamp(0, 1)

    market_caps.transform_values do |cap|
      # A total of zero means nothing is known about relative size; equal weight is the honest
      # answer there, rather than a division by zero.
      market_cap_weight = total.positive? ? cap.to_f / total : equal_weight
      (market_cap_weight * (1 - flattening)) + (equal_weight * flattening)
    end
  end
end
