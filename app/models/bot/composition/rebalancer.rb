# A composition bot's slice of the shared rebalance machinery (Bot::Rebalancer): its target list.
module Bot::Composition::Rebalancer
  extend ActiveSupport::Concern

  include Bot::Rebalancer

  private

  # Membership and weights are refreshed by the DCA tick — which a STOPPED bot never
  # runs. Since rebalancing deliberately keeps working while the schedule is stopped, without this
  # such a bot would rebalance forever toward a composition frozen at the moment it was stopped,
  # holding assets the composition has dropped and never buying the ones it has added.
  #
  # Best effort: a failed refresh leaves the stored composition in place rather than stopping a
  # rebalance that is probably still correct. (Bot::Composition::Liquidatable is stricter, because
  # there a stale composition would SELL an asset that may have re-entered.)
  def before_rebalance
    result = refresh_composition
    Rails.logger.warn("rebalance composition refresh failed bot=#{id}: #{result.errors.to_sentence}") if result.failure?
  end

  # Only assets the composition currently wants. Rebalancing tracks the composition, so an asset that
  # has LEFT it is not a member to steer toward a weight — it is a position to close, and closing it
  # is the user's call (Bot::Composition::Liquidatable), not a side effect of some other asset
  # breaching the band.
  #
  # Excluded from the DENOMINATOR too, not just the candidates: the weights describe the composition,
  # so an outside holding must not dilute them. Leaving exited holdings in with target 0 also made them
  # permanently the most-overweight entry, which sold them automatically — and an asset that left at
  # 0.1% of the portfolio never tripped a 5-point band at all, so it was never sold either. Neither
  # behaviour was wanted.
  def rebalance_targets
    data = metrics_with_current_prices
    return nil if data[:prices_stale]

    values = data[:asset_values] || {}
    tickers_by_symbol = tickers.index_by(&:base)
    composition_assets = bot_index_assets.in_index.includes(:asset).to_a
    return nil if composition_assets.empty?

    # A locked member is treated exactly like a quitter here — see Bot::WashSaleGuard.
    unlocked = composition_assets.reject(&:buy_locked?)
    return nil if unlocked.empty?

    in_index_symbols = unlocked.to_set { |bia| bia.asset.symbol }
    return nil if unpriced_holding?(data, values, tickers_by_symbol, in_index_symbols)

    entries = unlocked.map do |bia|
      symbol = bia.asset.symbol
      {
        ticker: tickers_by_symbol[symbol],
        value: values.dig(symbol, :current_value).to_d,
        # nil weight means we do not know what this asset is supposed to be — resolved below to
        # "whatever it currently is", never to 0.
        target: bia.target_allocation&.to_d
      }
    end

    normalise_targets(entries)
  end

  # A bulk price response can come back successful but incomplete, and metrics_with_current_prices
  # simply skips a symbol it could not price — WITHOUT raising the stale flag. Valuing such a holding
  # at zero would manufacture drift out of nothing: the bot would read the asset as worthless, sell
  # others to fund it and buy more of it. Defer instead.
  #
  # Scoped to current symbols that still have a tradeable ticker. An exited holding no longer takes part in
  # the arithmetic, so its price is nobody's business here; and a genuinely delisted holding — which
  # can never be priced again — must not wedge rebalancing forever.
  def unpriced_holding?(data, values, tickers_by_symbol, in_index_symbols)
    (data[:asset_breakdown] || {}).any? do |symbol, holding|
      in_index_symbols.include?(symbol) &&
        holding[:amount].to_d.positive? &&
        !values.key?(symbol) &&
        tickers_by_symbol.key?(symbol)
    end
  end

  # One normalisation, so the targets always sum to one whatever was removed (a quitter, a locked
  # member) or never recorded. A member whose weight the composition never wrote keeps its own
  # current share of the survivors' value, so it reads as exactly on target: never the most
  # overweight, never the most underweight, and above all never sold to zero on the strength of a
  # missing number. The recorded weights of the other survivors are then scaled to fill whatever
  # mass is left — the same rule whether what is missing is a locked name's weight or an unknown.
  def normalise_targets(entries)
    total_value = entries.sum { |entry| entry[:value].to_d }
    unknown, known = entries.partition { |entry| entry[:target].nil? }
    unknown.each { |entry| entry[:target] = total_value.positive? ? entry[:value].to_d / total_value : 0.to_d }

    # The mass left for the recorded weights; zero when the unknowns hold every unit of value, in
    # which case the recorded weights scale to zero rather than being left to sum past one.
    known_mass = [1 - unknown.sum { |entry| entry[:target] }, 0.to_d].max
    known_total = known.sum { |entry| entry[:target].to_d }
    known.each { |entry| entry[:target] = entry[:target].to_d * known_mass / known_total } if known_total.positive?
    entries
  end
end
