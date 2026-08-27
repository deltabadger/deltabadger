require 'test_helper'

class TrackerHelperTest < ActionView::TestCase
  include ApplicationHelper
  include ColorsHelper

  Holding = Struct.new(:asset, :value)

  def setup
    @denomination = Denomination.new('USD', 1.to_d)
  end

  def holding(symbol, category, value)
    Holding.new(Asset.new(symbol: symbol, category: category), value.to_d)
  end

  test 'type shares read by value, largest first, stablecoins apart from crypto' do
    holdings = [holding('BTC', 'Cryptocurrency', 80), holding('AAPL', 'Stock', 10),
                holding('USDT', 'Cryptocurrency', 10)]

    assert_equal [['Crypto', 80], ['Stock', 10], ['Stable', 10]], holdings_type_shares(holdings)
  end

  test 'no shares without value, and none that round to zero' do
    assert_empty holdings_type_shares([holding('BTC', 'Cryptocurrency', 0)])
    assert_equal [['Crypto', 100]],
                 holdings_type_shares([holding('BTC', 'Cryptocurrency', 10_000), holding('AAPL', 'Stock', 1)])
  end

  # == money is written to the cent, here too ==
  #
  # The bot tables round money to the cent whatever the venue publishes; the tracker kept eight
  # places, so the same USDC balance read 0.22812533 on one page and 0.23 on the other. The UNIT
  # decides: a figure written in a currency is written in its cents, whatever it is a figure OF.
  # A quantity of a coin still keeps eight — two would round most tokens away.
  test 'a quantity of a stablecoin is written to the cent' do
    assert_equal '0.23', tracker_amount(0.22812533.to_d, 'USDC')
  end

  test 'a figure in fiat keeps its cents, trailing zeros and all' do
    assert_equal '0.20', tracker_amount(0.2.to_d, 'EUR')
    assert_equal '0.00', tracker_amount(0.003.to_d, 'EUR')
  end

  test 'a quantity of a coin keeps the decimals it is published with' do
    assert_equal '0.00000008', tracker_amount(0.00000008.to_d, 'BTC')
    assert_equal '0.00000008', tracker_amount(0.00000008.to_d)
  end

  test 'a price written in a currency is written in its cents, coin or not' do
    assert_equal '0.09 <small>USDC</small>', tracker_figure(0.09078826.to_d, 'USDC')
    assert_equal '0.86 <small>EUR</small>', tracker_figure(0.859107.to_d, 'EUR')
  end

  test 'a fee in a coin keeps its decimals' do
    assert_equal '0.00000008 <small>BTC</small>', tracker_figure(0.00000008.to_d, 'BTC')
  end
  # == the menu icon is this ring, small ==
  #
  # Same colours, same order, same direction as the holdings card — read off the balances alone,
  # because share is all a 24px ring says and only cost needs the ledger.
  def icon_balance(user, color, value)
    AccountBalance.create!(user: user, exchange: (@icon_exchange ||= create(:binance_exchange)),
                           asset: create(:asset, color: color), free: 1, locked: 0,
                           usd_price: value, usd_value: value, synced_at: Time.current)
  end

  # The share the slice stands for. Only valid for a slice wide enough to take the whole gap — a
  # tighter one is a bare dot, and its dash carries no width to read a share back out of.
  def arc_share(arc)
    visible, rest = arc[:dash].split.map(&:to_f)
    (visible + TrackerHelper::ICON_GAP + TrackerHelper::ICON_STROKE) / (visible + rest)
  end

  # Where the slice's span begins, as a fraction of the ring clockwise from twelve o'clock. Same
  # caveat as `arc_share`.
  def arc_start(arc)
    inset = (TrackerHelper::ICON_GAP + TrackerHelper::ICON_STROKE) / 2
    ((-arc[:offset] - inset) / TrackerHelper::ICON_CIRCUMFERENCE) + 0.25
  end

  # What survives the fold, as [percent, colour] — the composition, before any of it is drawn.
  def icon_shares(pairs)
    slices = icon_slices(pairs.map { |value, color| [value.to_d, color] })
    total = slices.sum(0.to_d) { |value, _| value }
    slices.map { |value, color| [(value / total * 100).round(1).to_f, color] }
  end

  test 'nothing to divide, no arcs: the icon stays a plain circle' do
    user = create(:user)

    assert_empty allocation_icon_arcs(user)

    icon_balance(user, '#F7931A', 0)

    assert_empty allocation_icon_arcs(user)
  end

  test 'the icon divides the ring by value, in the holdings colours, largest first' do
    user = create(:user)
    icon_balance(user, '#627EEA', 25)
    icon_balance(user, '#F7931A', 75)

    arcs = allocation_icon_arcs(user)

    assert_equal([ensure_contrast('#F7931A'), ensure_contrast('#627EEA')], arcs.map { |arc| arc[:color] })
    assert_in_delta 0.75, arc_share(arcs.first), 0.001
    assert_in_delta 0.25, arc_share(arcs.last), 0.001
    # Clockwise from twelve o'clock, each span starting exactly where the one before it ended.
    assert_in_delta 0.0, arc_start(arcs.first), 0.001
    assert_in_delta 0.75, arc_start(arcs.last), 0.001
  end

  test 'the same coin on two venues is one arc' do
    user = create(:user)
    btc = create(:asset, :bitcoin, color: '#F7931A')
    [create(:binance_exchange), create(:kraken_exchange)].each do |exchange|
      AccountBalance.create!(user: user, exchange: exchange, asset: btc, free: 1, locked: 0,
                             usd_price: 50, usd_value: 50, synced_at: Time.current)
    end

    assert_equal 1, allocation_icon_arcs(user).size
  end

  # == what is drawn, and what is gathered ==
  #
  # The floor is the stroke, not the gap: a round-capped dash of nothing is already a dot as wide as
  # the stroke, and a slice too tight for the whole gap keeps whatever is left of its span. So only
  # the genuine tail is gathered, and everything the ring can draw keeps its own colour.
  test 'a holding too small for the gap is still drawn, as a dot' do
    arcs = icon_arcs([[96, '#4C6B4C'], [4, '#1652F0']].map { |value, color| [value.to_d, color] })

    assert_equal 2, arcs.size
    # No dash left at all: what is drawn is the two round caps meeting, a dot as wide as the stroke.
    assert_equal '0.0', arcs.last[:dash].split.first
    assert_in_delta 0.96, arc_share(arcs.first), 0.001
  end

  # What the icon used to get wrong: it dropped everything it could not draw, handed that share to
  # the largest holding and drew the whole ring in one colour. The tail is gathered now — but it is
  # only the tail, so the two small holdings the card shows keep their own colours here too.
  test 'only the undrawable tail is gathered, into one neutral slice, last' do
    user = create(:user)
    icon_balance(user, '#4C6B4C', 78)
    icon_balance(user, '#1652F0', 4.5)
    icon_balance(user, '#F7931A', 4)
    9.times { icon_balance(user, '#888888', 1.5) }

    arcs = allocation_icon_arcs(user)

    assert_equal([ensure_contrast('#4C6B4C'), ensure_contrast('#1652F0'), ensure_contrast('#F7931A'),
                  ensure_contrast(TrackerHelper::NEUTRAL_COLOR)],
                 arcs.map { |arc| arc[:color] })
    assert_equal 13.5, icon_shares([[78, '#4C6B4C'], [4.5, '#1652F0'], [4, '#F7931A']] +
                                   Array.new(9) { [1.5, '#888888'] }).last.first
  end

  test 'a tail too thin even to gather is dropped, and the rest read as the whole' do
    assert_equal [[100.0, '#F7931A']],
                 icon_shares([[98, '#F7931A'], [1, '#627EEA'], [1, '#26A17B']])
  end

  # One holding is not an "other": there is nothing to gather it with, and it cannot be drawn at
  # this size either, so it goes and the rest is the whole.
  test 'a single holding below the floor is dropped rather than named a remainder' do
    assert_equal [[60.6, '#F7931A'], [39.4, '#627EEA']],
                 icon_shares([[60, '#F7931A'], [39, '#627EEA'], [1, '#26A17B']])
  end

  # With nothing above the floor there is nothing for a tail to be "other" than, and one neutral
  # ring is the icon for a portfolio nobody has synced. So the largest that fit are shown instead,
  # dropped one at a time — dropping one lifts every other share, so the floor is re-read after each.
  test 'with no holding above the floor, the largest that fit are shown' do
    arcs = icon_arcs(Array.new(40) { [2.5.to_d, '#F7931A'] })

    assert_equal 28, arcs.size
    assert_operator TrackerHelper::ICON_CIRCUMFERENCE / arcs.size, :>=, TrackerHelper::ICON_MIN_SPAN
  end

  test 'every slice keeps its whole share: the spans meet, and they close the ring' do
    arcs = icon_arcs([[60, '#F7931A'], [25, '#627EEA'], [15, '#26A17B']].map { |value, color| [value.to_d, color] })

    assert_in_delta 1.0, arcs.sum { |arc| arc_share(arc) }, 0.001
    assert_in_delta 0.6, arc_start(arcs[1]), 0.001
    assert_in_delta 0.85, arc_start(arcs[2]), 0.001
  end
end
