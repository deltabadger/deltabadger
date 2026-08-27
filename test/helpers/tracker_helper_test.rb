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

  # The share the slice stands for: the dash is shorter than its span by one gap and one round cap.
  def arc_share(arc)
    visible, rest = arc[:dash].split.map(&:to_f)
    (visible + TrackerHelper::ICON_MIN_SPAN) / (visible + rest)
  end

  # Where the slice's span begins, as a fraction of the ring clockwise from twelve o'clock.
  def arc_start(arc)
    ((-arc[:offset] - (TrackerHelper::ICON_MIN_SPAN / 2)) / TrackerHelper::ICON_CIRCUMFERENCE) + 0.25
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

  # A round-capped dash of nothing is already a dot as wide as the stroke: below a gap plus a stroke
  # there is no room left to draw, so the slice goes rather than shrinking into its neighbours.
  test 'a slice with no room to be drawn is dropped, and the rest read as the whole' do
    user = create(:user)
    icon_balance(user, '#F7931A', 98)
    icon_balance(user, '#627EEA', 1)
    icon_balance(user, '#26A17B', 1)

    arcs = allocation_icon_arcs(user)

    assert_equal 1, arcs.size
    assert_in_delta 1.0, arc_share(arcs.first), 0.001
  end

  # Dropping one lifts every other share, so the floor has to be re-read after each: twenty equal
  # holdings are 5% each and too thin, and only stop being thin once six of them have gone.
  test 'the smallest go one at a time, until the smallest left has room' do
    arcs = icon_arcs(Array.new(20) { [5.to_d, '#F7931A'] })

    assert_equal 14, arcs.size
    assert_operator arc_share(arcs.last) * TrackerHelper::ICON_CIRCUMFERENCE, :>=, TrackerHelper::ICON_MIN_SPAN
  end

  test 'every slice keeps its whole share: the spans meet, and they close the ring' do
    arcs = icon_arcs([[60, '#F7931A'], [25, '#627EEA'], [15, '#26A17B']].map { |value, color| [value.to_d, color] })

    assert_in_delta 1.0, arcs.sum { |arc| arc_share(arc) }, 0.001
    assert_in_delta 0.6, arc_start(arcs[1]), 0.001
    assert_in_delta 0.85, arc_start(arcs[2]), 0.001
  end
end
