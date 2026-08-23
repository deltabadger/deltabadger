module TrackerHelper
  # The allocation ring. Flat, one arc per holding, drawn server-side: it is a picture of numbers
  # the page already has, so there is nothing for a chart library to fetch or animate.
  RING_RADIUS = 100
  RING_CENTRE = 115
  RING_GAP_DEGREES = 2.6
  # Under 2% a 14px stroke cannot draw an arc as anything but a cap, and caps overlap their
  # neighbours — so the tail folds into one neutral slice instead.
  RING_MIN_SHARE = 0.02
  NEUTRAL_COLOR = '#8A9BA8'.freeze
  # Exchanges round dust differently, so the ledger and the balance never match to the last digit.
  # A gap wider than this is missing history, not rounding, and no P/L beats a wrong one.
  RECONCILE_TOLERANCE = 0.02

  # [{ d:, color:, label: }] in the order the rows are listed.
  def holdings_ring_arcs(slices)
    total = slices.sum { |slice| slice[:usd_value].to_d }
    return [] unless total.positive?

    small, large = slices.partition { |slice| slice[:usd_value].to_d / total < RING_MIN_SHARE }
    arcs = large.map do |slice|
      [slice[:usd_value].to_d, ensure_contrast(slice[:asset].color.presence || NEUTRAL_COLOR), slice[:asset].symbol]
    end
    folded = small.sum { |slice| slice[:usd_value].to_d }
    arcs << [folded, NEUTRAL_COLOR, t('tracker.other')] if folded.positive?

    angle = 0.0
    arcs.map do |value, color, label|
      sweep = (value / total).to_f * 360
      gap = [RING_GAP_DEGREES, sweep / 2].min
      arc = { d: ring_arc_path(angle + (gap / 2), angle + sweep - (gap / 2)), color: color, label: label }
      angle += sweep
      arc
    end
  end

  # A holding's P/L against the ledger's remaining FIFO cost — but only when the two agree about how
  # much is held. Reconciled HERE, at render time, and never inside the cached ledger: a balance sync
  # changes the quantity and must not have to invalidate a ledger it cannot otherwise affect.
  # nil means "say nothing": cash, an asset the ledger never saw, or a quantity that disagrees.
  def holding_pnl_percent(slice, position)
    cost = holding_cost(slice, position)
    return unless cost

    ((slice[:usd_value].to_d / cost) - 1) * 100
  end

  # The card's centre figure: the same reading over every holding that reconciles, value-weighted.
  def holdings_total_pnl_percent(slices, positions)
    reconciled = slices.filter_map do |slice|
      cost = holding_cost(slice, positions[slice[:asset].symbol])
      [slice[:usd_value].to_d, cost] if cost
    end
    cost = reconciled.sum(0.to_d) { |_value, basis| basis }
    return unless cost.positive?

    ((reconciled.sum(0.to_d) { |value, _basis| value } / cost) - 1) * 100
  end

  # "Crypto" / "Stock" / "ETF" / "Cash", plus the one distinction the asset table does not carry:
  # a stablecoin is not a position, it is the cash a position was bought with.
  def holding_type_label(asset)
    return 'Stable' if Tax::PriceService::STABLECOINS.include?(asset.symbol)

    asset_type_label(asset.category)
  end

  # [value, invested] for the chart, in the display currency.
  #
  # While balances are hidden the two curves are NORMALIZED instead: every point divided by the last
  # invested figure, which stands the invested line at 100 and reads value against it. The shape is
  # identical and the payload carries proportions rather than money — the attribute is on the page
  # for anyone who opens the inspector, so it has to be as quiet as the pixels. The divisor falls
  # back to the last VALUE when nothing was ever invested, and to 1 when both are zero, so what
  # ships is always finite.
  def chart_history_series(history, hide_money)
    values = history.map { |row| row.value_usd.to_d }
    invested = history.map { |row| row.invested_usd.to_d }
    return denominated_series(values, invested) unless hide_money

    divisor = [invested.last, values.last, 1.to_d].find(&:positive?)
    [values, invested].map { |serie| serie.map { |amount| ((amount / divisor) * 100).round(2).to_f } }
  end

  # The record's second pane: one row per open position, one per closed round-trip, newest first.
  # Two shapes in one table — what they have in common is a cost, an exit and a holding period.
  # An open row is marked at the balance's market price; a closed one at what it actually sold for.
  def tracker_position_rows(ledger, slices)
    return [] unless ledger

    by_symbol = slices.index_by { |slice| slice[:asset].symbol }
    rows = ledger.positions.map { |position| open_position_row(position, by_symbol[position.symbol]) } +
           ledger.round_trips.map { |trip| round_trip_row(trip) }
    rows.sort_by { |row| row[:opened] || Time.at(0) }.reverse
  end

  # A signed percentage, one decimal — the reading on every P/L cell on the page.
  def tracker_percent(percent)
    "#{'+' if percent >= 0}#{number_with_precision(percent, precision: 1)}%"
  end

  # A quantity or a price. Two decimals once it is worth more than a unit, eight below that, where
  # two would round the whole number away.
  def tracker_amount(value)
    return unless value

    value = value.to_d
    return number_with_precision(value, precision: 2, delimiter: ',') if value.abs >= 1

    number_with_precision(value, precision: 8, strip_insignificant_zeros: true)
  end

  # How long a position was held, in the largest unit that still says something.
  def tracker_holding_period(from, to)
    days = ((to - from) / 1.day).to_i
    return "#{days}d" if days < 60
    return "#{(days / 30.0).round}mo" if days < 365

    "#{number_with_precision(days / 365.0, precision: 1, strip_insignificant_zeros: true)}y"
  end

  # symbol → Asset for the rows on this page: the user's own balance row first (the asset the rest
  # of the page draws this symbol with), then the crypto asset of that ticker. Primed once from the
  # listed transactions; the single-row fallback is for the transfer toggle, which re-renders one
  # row through the same partial with no page around it.
  def tracker_row_asset(symbol)
    @tracker_row_assets ||= Tracker::Ledger.asset_index(current_user, tracker_row_symbols)
    return @tracker_row_assets[symbol] if @tracker_row_assets.key?(symbol)

    @tracker_row_assets[symbol] = Tracker::Ledger.asset_index(current_user, [symbol])[symbol]
  end

  # The tab a transaction row belongs to, in the bot log's vocabulary. Token membership, so `all`
  # rides on every row — the order-filter controller tests membership, not equality.
  def tracker_row_types(transaction)
    return 'all transfer' if transaction.linked?
    return "all #{transaction.entry_type}" if transaction.buy? || transaction.sell?

    'all other'
  end

  private

  def denominated_series(values, invested)
    [values, invested].map { |serie| serie.map { |amount| @denomination.convert(amount).round(2).to_f } }
  end

  def open_position_row(position, slice)
    quantity = slice ? slice[:quantity].to_d : position.quantity
    percent = slice && holding_pnl_percent(slice, position)
    { status: 'open', symbol: position.symbol, asset: position.asset, opened: position.opened_at, closed: nil,
      invested: position.cost_usd, avg_buy: position.avg_cost_usd,
      price: (slice && quantity.positive? ? slice[:usd_value].to_d / quantity : nil),
      percent: percent,
      cash: percent && (slice[:usd_value].to_d - (position.avg_cost_usd * quantity)) }
  end

  def round_trip_row(trip)
    { status: trip.realised_pnl_usd.negative? ? 'loss' : 'win', symbol: trip.symbol, asset: trip.asset,
      opened: trip.opened_at, closed: trip.closed_at, invested: trip.invested_usd,
      avg_buy: (trip.invested_usd / trip.quantity if trip.quantity.positive?),
      price: (trip.proceeds_usd / trip.quantity if trip.quantity.positive?),
      percent: (trip.realised_pnl_usd / trip.invested_usd * 100 if trip.invested_usd.positive?),
      cash: trip.realised_pnl_usd }
  end

  def tracker_row_symbols
    (@account_transactions || []).map(&:base_currency).uniq
  end

  def holding_cost(slice, position)
    quantity = slice[:quantity].to_d
    return if position.nil? || !quantity.positive? || !position.quantity.positive? || !position.cost_usd.positive?
    return if ((position.quantity - quantity) / quantity).abs > RECONCILE_TOLERANCE

    position.avg_cost_usd * quantity
  end

  def ring_arc_path(from, to)
    start_x, start_y = ring_point(from)
    end_x, end_y = ring_point(to)
    "M #{start_x} #{start_y} A #{RING_RADIUS} #{RING_RADIUS} 0 #{to - from > 180 ? 1 : 0} 1 #{end_x} #{end_y}"
  end

  def ring_point(degrees)
    radians = degrees * Math::PI / 180
    [(RING_CENTRE + (RING_RADIUS * Math.cos(radians))).round(2),
     (RING_CENTRE + (RING_RADIUS * Math.sin(radians))).round(2)]
  end
end
