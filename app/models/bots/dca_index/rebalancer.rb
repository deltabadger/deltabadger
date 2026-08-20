# The index bot's slice of the shared rebalance machinery (Bot::Rebalancer): its target list.
module Bots::DcaIndex::Rebalancer
  extend ActiveSupport::Concern

  include Bot::Rebalancer

  private

  # The index's membership and weights are refreshed by the DCA tick — which a STOPPED bot never
  # runs. Since rebalancing deliberately keeps working while the schedule is stopped, without this
  # such a bot would rebalance forever toward a composition frozen at the moment it was stopped,
  # holding constituents the index has dropped and never buying the ones it has added.
  #
  # Best effort: a failed refresh leaves the stored composition in place rather than stopping a
  # rebalance that is probably still correct.
  def before_rebalance
    result = refresh_index_composition
    Rails.logger.warn("rebalance composition refresh failed bot=#{id}: #{result.errors.to_sentence}") if result.failure?
  end

  # Every asset the bot holds, plus every asset the index currently wants — the union, because both
  # sides matter: an asset held but no longer in the index has to be sellable, and an asset newly
  # admitted to the index has to be buyable before the bot owns any of it.
  #
  # Assets that have LEFT the index carry target 0. That makes them maximally overweight, so
  # rebalancing liquidates them and redistributes the proceeds — which is what tracking an index
  # means once its composition changes. Until rebalancing is switched on they are simply held, as
  # before: the DCA leg stopped buying them the moment they exited.
  def rebalance_targets
    data = metrics_with_current_prices
    return nil if data[:prices_stale]

    values = data[:asset_values] || {}
    tickers_by_symbol = tickers.index_by(&:base)
    # A bulk price response can come back successful but incomplete, and metrics_with_current_prices
    # simply skips a symbol it could not price — WITHOUT raising the stale flag. Valuing such a
    # holding at zero would manufacture drift out of nothing: the bot would read the asset as
    # worthless, sell others to fund it and buy more of it. Defer instead.
    #
    # Scoped to symbols that still have a tradeable ticker, so a genuinely delisted holding — which
    # can never be priced again — does not wedge rebalancing forever.
    unpriced = (data[:asset_breakdown] || {}).any? do |symbol, holding|
      holding[:amount].to_d.positive? && !values.key?(symbol) && tickers_by_symbol.key?(symbol)
    end
    return nil if unpriced

    targets_by_asset_id = bot_index_assets.in_index.pluck(:asset_id, :target_allocation).to_h

    entries = bot_index_assets.includes(:asset).map do |bia|
      symbol = bia.asset.symbol
      weight = targets_by_asset_id[bia.asset_id]
      {
        ticker: tickers_by_symbol[symbol],
        value: values.dig(symbol, :current_value).to_d,
        # nil weight while still IN the index means we do not know what this asset is supposed to
        # be — resolved below to "whatever it currently is", never to 0. Only an asset that has
        # actually LEFT gets target 0, because only there is liquidation the intended answer.
        target: targets_by_asset_id.key?(bia.asset_id) ? weight&.to_d : 0.to_d
      }
    end

    # Holdings from a composition the bot no longer tracks at all (an asset whose BotIndexAsset row
    # predates a change, or a symbol the breakdown knows and the index does not) still belong to the
    # portfolio, so they count toward the total and are sellable at target 0.
    known = entries.filter_map { |entry| entry[:ticker]&.base }.to_set
    values.each do |symbol, asset_data|
      next if known.include?(symbol)

      entries << { ticker: tickers_by_symbol[symbol], value: asset_data[:current_value].to_d, target: 0.to_d }
    end

    resolve_unknown_targets(entries)
  end

  # An in-index asset whose weight the composition never wrote gets its own current share, so it
  # reads as exactly on target: never the most overweight, never the most underweight, and above all
  # never sold to zero on the strength of a missing number.
  def resolve_unknown_targets(entries)
    return entries if entries.none? { |entry| entry[:target].nil? }

    total = entries.sum { |entry| entry[:value] }
    entries.each do |entry|
      next unless entry[:target].nil?

      entry[:target] = total.positive? ? entry[:value] / total : 0.to_d
    end
    entries
  end
end
