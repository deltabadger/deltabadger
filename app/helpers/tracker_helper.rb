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
  def holdings_ring_arcs(holdings)
    total = holdings.sum(0.to_d, &:value)
    return [] unless total.positive?

    small, large = holdings.partition { |holding| holding.value / total < RING_MIN_SHARE }
    arcs = large.map do |holding|
      [holding.value, ensure_contrast(holding.asset.color.presence || NEUTRAL_COLOR), holding.asset.symbol]
    end
    folded = small.sum(0.to_d, &:value)
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
    value, cost = reconciled_holdings(slices, positions)
    return unless cost.positive?

    ((value / cost) - 1) * 100
  end

  # The same reading in money, for the grid. Percent and money have to come from ONE set of holdings
  # or the two would state different things under different names — which is the confusion the grid
  # exists to end.
  def holdings_unrealised_usd(slices, positions)
    value, cost = reconciled_holdings(slices, positions)
    return unless cost.positive?

    value - cost
  end

  # The tone of a chip, by meaning rather than by domain — see `new/_pill.sass`. Anything the map
  # does not place is a label, not a state, and reads as one.
  PILL_TONES = {
    'buy' => 'up', 'swap_in' => 'up', 'win' => 'up',
    'sell' => 'down', 'swap_out' => 'down', 'lost' => 'down', 'loss' => 'down',
    'cash' => 'quiet',
    'deposit' => 'info', 'staking_reward' => 'info', 'lending_interest' => 'info', 'airdrop' => 'info',
    'mining' => 'info', 'other_income' => 'info', 'return_of_capital' => 'info', 'open' => 'info',
    'withdrawal' => 'warn'
  }.freeze

  def pill_class(kind)
    "pill pill--#{PILL_TONES.fetch(kind.to_s, 'quiet')}"
  end

  # "Crypto" / "Stock" / "ETF" / "Cash", plus the one distinction the asset table does not carry:
  # a stablecoin is not a position, it is the cash a position was bought with.
  def holding_type_label(asset)
    return 'Stable' if Tax::PriceService::STABLECOINS.include?(asset.symbol)

    asset_type_label(asset.category)
  end

  # The holdings header: what share of the portfolio each KIND of asset is, by value — "Stocks 10% ·
  # Crypto 80% · Stable 10%". Same labels the rows carry, so it reads as a summary of the list below
  # rather than a second opinion. Shares that round to nothing are left out; a "0%" is noise.
  def holdings_type_shares(holdings)
    total = holdings.sum(0.to_d, &:value)
    return [] unless total.positive?

    holdings.group_by { |holding| holding_type_label(holding.asset) || t('tracker.other') }
            .map { |label, group| [label, (group.sum(0.to_d, &:value) / total * 100).round] }
            .reject { |_, share| share.zero? }
            .sort_by { |_, share| -share }
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
  # Built from what is HELD, not from what the ledger keeps a position in — those are different
  # sets, and the page used to show one in this table and the other on the card above it. Cash is
  # the case that made it visible: it is a balance, so the card lists it, and the ledger keeps no
  # position for it because it has no cost and no gain, so the table did not.
  #
  # A coin the venue has STOPPED reporting is not a row: it left at cost, and the note under the
  # holdings says so.
  def tracker_position_rows(figures)
    return [] unless figures&.ledger

    @tracker_positions = figures.ledger.positions.index_by(&:symbol)
    held = figures.holdings.map { |holding| held_row(holding, @tracker_positions[holding.asset.symbol]) }

    (held + figures.ledger.round_trips.map { |trip| round_trip_row(trip) })
      .sort_by { |row| row[:opened] || Time.at(0) }.reverse
  end

  # An assumption, in words: the exchange's name (or "your exchanges"), both quantities, and what
  # it moved — masked when balances are hidden, since that amount is money.
  def tracker_note(note)
    hidden = current_user.hide_balances?
    # A cash note's quantities are money too.
    quantity = ->(units) { hidden && money?(note.symbol) ? '•••' : tracker_amount(units) }
    t("tracker.notes.#{note.kind}", symbol: note.symbol, exchange: note.exchange || t('tracker.notes.your_exchanges'),
                                    history: quantity.call(note.history), held: quantity.call(note.held),
                                    amount: hidden ? '•••' : @denomination.format_plain(note.amount_usd))
  end

  # Whose price a row carries, and what it is — [price, source]:
  #
  #   :exchange — the venue's own, as it booked it, in the venue's own currency: a string, shown as
  #               is. A fact of the record, like Amount and Fee.
  #   :stated   — the user typed it, per unit. Stored in USD; shown in the reader's currency.
  #   :ours     — the venue booked none, so we priced it from our own history. The default, and
  #               the thing the figures are actually built on for a dust rebate, an airdrop, a
  #               withdrawal. Shown in the reader's currency, like Value beside it.
  #   :cash     — the row's base is money, and the price of money is its rate: what one unit bought
  #               that day, in the reader's currency. nil where no rate exists for that day.
  #   nil       — nobody could price it. This is where the figures go silent, and where a typed
  #               price is worth more than any amount of retrying.
  #
  # Deliberately reads only prices ALREADY STORED: this runs once per row of a 200-row table, and
  # the stored table is exactly what the ledger's own calculations used.
  #
  # Over the row's SIZE: a split is one signed net delta, and a reverse split has a price like any
  # other row — the sign belongs to the amount, and Value carries it. Only a row of nothing has none.
  def tracker_row_price(record, denomination)
    return [in_display(1.to_d, record.base_currency, record, denomination), :cash] if money?(record.base_currency)

    amount = record.base_amount.to_d.abs
    return [nil, nil] if amount.zero?

    if record.quoted?
      [tracker_figure(record.quote_amount.to_d / amount, record.quote_currency), :exchange]
    elsif (cash = with_group(record).cash_counterpart)
      [tracker_figure(cash.base_amount.to_d / amount, cash.base_currency), :exchange]
    elsif (stated = record.manual_value(:price))
      [denomination.convert(stated), :stated]
    else
      price = HistoricalPrice.lookup(asset: record.base_currency, currency: 'USD', date: record.transacted_at.to_date)
      price&.positive? ? [denomination.convert(price.to_d), :ours] : [nil, nil]
    end
  end

  # What a row was worth, in the DENOMINATION's currency — the one unit that column is written in —
  # or nil where nothing could price it, which the column says rather than guessing. Never typed:
  # amount times price, in the same currency the price is shown in, so the two visibly multiply
  # out. The venue's own counter-amount is the exception, and is carried across at the rate OF ITS
  # OWN DAY — five euro sixty-one is five euro sixty-one, and round-tripping it through dollars at
  # today's rate hands back five seventy-one.
  def tracker_row_value(record, denomination, price, source)
    case source
    when :exchange
      if record.quoted?
        in_display(record.quote_amount, record.quote_currency, record, denomination)
      else
        cash = record.cash_counterpart
        in_display(cash.base_amount, cash.base_currency, record, denomination)
      end
    when :cash, :stated, :ours then price && (price * record.base_amount.to_d)
    end
  end

  # The page's rows hand their groups to each other in one query, so a row does not ask for its
  # own. A row rendered on its own (a turbo stream) is not in the page's set and asks itself.
  def with_group(record)
    key = [record.exchange_id, record.group_id]
    record.group_rows = tracker_row_groups[key] if record.group_id.present? && tracker_row_groups.key?(key)
    record
  end

  def tracker_row_groups
    @tracker_row_groups ||= begin
      rows = (@account_transactions || []).to_a
      keys = rows.filter_map { |row| [row.exchange_id, row.group_id] if row.group_id.present? }.uniq
      if keys.empty?
        {}
      else
        AccountTransaction.for_user(current_user)
                          .where(exchange_id: keys.map(&:first).uniq, group_id: keys.map(&:last).uniq)
                          .group_by { |row| [row.exchange_id, row.group_id] }
      end
    end
  end

  # A signed percentage, one decimal — the reading on every P/L cell on the page.
  def tracker_percent(percent)
    "#{'+' if percent >= 0}#{number_with_precision(percent, precision: 1)}%"
  end

  # A figure and the unit it is in, the unit in <small>: the number is what the column is read for.
  def tracker_figure(value, unit)
    safe_join([tracker_amount(value), ' ', tag.small(unit)])
  end

  # A quantity or a price. Two decimals once it is worth more than a unit, eight below that, where
  # two would round the whole number away.
  def tracker_amount(value)
    return unless value

    value = value.to_d
    return number_with_precision(value, precision: 2, delimiter: ',') if value.abs >= 1

    number_with_precision(value, precision: 8, strip_insignificant_zeros: true)
  end

  # "2 days", not "2 days, 8 hours, and 47 minutes" — dotiw's full form is right inside a sentence
  # and far too long for a label sitting in a bar next to a button.
  def tracker_synced_ago(time)
    t('tracker.synced_ago', ago: time_ago_in_words(time, highest_measures: 1))
  end

  # A price in the display currency. Two decimals above a unit, eight below it — an average buy of
  # $0.00 is what a two-decimal rule makes of every sub-dollar coin, and it says nothing at all.
  # Zero is the exception: it is not a small number, it is no number, and eight decimals of nothing
  # is just noise in the column.
  def tracker_price(usd_amount)
    return unless usd_amount

    value = usd_amount.to_d
    @denomination.format(value, precision: value.nonzero? && value.abs < 1 ? 8 : 2)
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

  # One rule for every filter on this page, the bot log's own: offer the options that EXIST, and
  # disappear when fewer than two of them are a real choice. A control with one answer is furniture,
  # and an option with nothing behind it is worse than no option.
  def offered(options)
    options.count { |option| option[:value] != 'all' } > 1 ? options : []
  end

  # The types this page actually holds, in the ledger's own order, plus Transfer when a linked pair
  # is on it. A linked deposit answers to both, which is what makes either filter find it.
  def tracker_type_filters(transactions)
    present = transactions.map(&:entry_type).uniq
    options = [{ value: 'all', label: t('tracker.record.all'), active: true }]
    options += AccountTransaction.entry_types.keys.select { |type| present.include?(type) }
                                 .map { |type| { value: type, label: t("tracker.types.#{type}") } }
    options << { value: 'transfer', label: t('tracker.record.transfer') } if transactions.any?(&:linked?)
    offered(options)
  end

  # Open / Win / Loss / Closed, but only the ones the table has rows for.
  def tracker_status_filters(rows)
    present = rows.map { |row| row[:status] }.uniq
    offered([{ value: 'all', label: t('tracker.record.all'), active: true }] +
            %w[open win loss closed cash].select { |status| present.include?(status) }
                                    .map { |status| { value: status, label: t("tracker.record.#{status}") } })
  end

  # A window shorter than the history is a choice; one longer than it draws the same picture as ALL,
  # so an account three days old is offered nothing.
  def chart_range_options(history)
    span = (history.last.date - history.first.date).to_i
    windows = [['30', t('tracker.range.30d')], ['365', t('tracker.range.1y')]].select { |days, _| span > days.to_i }
    return [] if windows.empty?

    windows.map { |value, label| { value: value, label: label } } <<
      { value: 'all', label: t('tracker.range.all'), active: true }
  end

  # The filters a row answers to. Token membership, so `all` rides on every row — the order-filter
  # controller tests membership, not equality — and a linked deposit is BOTH a deposit and a
  # transfer, which is what makes either filter find it.
  def tracker_row_types(transaction)
    tokens = ['all', transaction.entry_type]
    tokens << 'transfer' if transaction.linked?
    tokens.join(' ')
  end

  private

  # A counter-amount only means a value when it is in money. Coin-for-coin, the row's worth still
  # has to be priced.
  # A stablecoin is taken at par against the dollar, as it is everywhere else in the app; real fiat
  # goes through the ECB's published rate FOR THAT DAY, straight to the currency on screen. No dollar
  # in the middle: a euro row shown in euro must come back as itself.
  def in_display(amount, currency, record, denomination)
    currency = 'USD' if Tax::PriceService::STABLECOINS.include?(currency)
    return amount.to_d if currency == denomination.currency

    rate = tracker_fx_rate(currency, denomination.currency, record.transacted_at.to_date)
    rate && (amount.to_d * rate)
  end

  # Memoised per render: a table of two hundred rows holds far fewer distinct currency-days, and the
  # misses are worth caching too — an unlisted currency would otherwise raise on every row.
  def tracker_fx_rate(from, to, date)
    @tracker_fx_rates ||= {}
    @tracker_fx_rates.fetch([from, to, date]) do
      @tracker_fx_rates[[from, to, date]] = begin
        Tax::EcbFxRates.rate(from: from, to: to, date: date)
      rescue Tax::EcbFxRates::MissingRate
        nil
      end
    end
  end

  def money?(currency)
    currency.present? &&
      (Tax::PriceService::FIAT_CURRENCIES.include?(currency) ||
       Tax::PriceService::STABLECOINS.include?(currency))
  end

  # [value, cost] over every holding whose quantity the ledger can vouch for. Cash and anything
  # unreconciled contribute nothing — there is no basis to divide by.
  def reconciled_holdings(slices, positions)
    pairs = slices.filter_map do |slice|
      cost = holding_cost(slice, positions[slice[:asset].symbol])
      [slice[:usd_value].to_d, cost] if cost
    end
    [pairs.sum(0.to_d) { |value, _| value }, pairs.sum(0.to_d) { |_, cost| cost }]
  end

  def denominated_series(values, invested)
    [values, invested].map { |serie| serie.map { |amount| @denomination.convert(amount).round(2).to_f } }
  end

  # Cash is stated as cash rather than dressed up as a position with nothing in its columns: it has
  # no cost to divide by and no gain to report, and saying so is shorter than five empty cells.
  def held_row(holding, _position)
    return open_position_row(holding) if holding.cost

    { status: 'cash', symbol: holding.asset.symbol, asset: holding.asset,
      opened: nil, closed: nil, invested: nil, avg_buy: nil,
      price: (holding.quantity.positive? ? holding.value / holding.quantity : nil),
      percent: nil, cash: nil }
  end

  # The holding is `Tracker::Figures`' own — resolved against the balance, cost at what is held —
  # so the row marks the position at the same value, with the same assumptions, as the tiles and
  # the card. It used to reconcile again here, on its own terms.
  def open_position_row(holding)
    position = @tracker_positions&.dig(holding.asset.symbol)
    { status: 'open', symbol: holding.asset.symbol, asset: holding.asset, opened: position&.opened_at, closed: nil,
      invested: holding.cost, avg_buy: (holding.quantity.positive? ? holding.cost / holding.quantity : nil),
      price: (holding.quantity.positive? ? holding.value / holding.quantity : nil),
      percent: holding.percent,
      cash: holding.unrealised }
  end

  # A trip whose basis was assumed anywhere along the way is CLOSED and nothing more: calling it a
  # win or a loss, to a decimal place, would state a figure the ledger cannot stand behind.
  def round_trip_row(trip)
    complete = !trip.incomplete && trip.invested_usd.positive?
    outcome = trip.realised_pnl_usd.negative? ? 'loss' : 'win'
    { status: trip.incomplete ? 'closed' : outcome,
      symbol: trip.symbol, asset: trip.asset, opened: trip.opened_at, closed: trip.closed_at,
      invested: trip.invested_usd,
      avg_buy: (trip.invested_usd / trip.quantity if trip.quantity.positive?),
      price: (trip.proceeds_usd / trip.quantity if trip.quantity.positive?),
      percent: (trip.realised_pnl_usd / trip.invested_usd * 100 if complete),
      cash: (trip.realised_pnl_usd if complete) }
  end

  def tracker_row_symbols
    (@account_transactions || []).map(&:base_currency).uniq
  end

  # The cost of what the venue says is there, at the ledger's average — but only when the two agree
  # about the quantity. They are read at different moments: the balance is a snapshot and the ledger
  # is current, so what the ledger has recorded since that snapshot (`@pending_quantities`) is added
  # to the balance before comparing. Without that, one bot buy is enough to make a holding look
  # unvouchable and take its P/L away. The cost and the value both stay the snapshot's.
  def holding_cost(slice, position)
    quantity = slice[:quantity].to_d
    return if position.nil? || position.incomplete
    return if !quantity.positive? || !position.quantity.positive? || !position.cost_usd.positive?

    expected = quantity + (@pending_quantities || {}).fetch(position.symbol, 0.to_d)
    return if !expected.positive? || ((position.quantity - expected) / expected).abs > RECONCILE_TOLERANCE

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
