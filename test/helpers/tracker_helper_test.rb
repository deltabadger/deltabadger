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

  # Where the slice's span begins, as a fraction of the ring clockwise from twelve o'clock.
  def arc_start(arc)
    ((-arc[:offset] - (TrackerHelper::ICON_MIN_SPAN / 2)) / TrackerHelper::ICON_CIRCUMFERENCE) + 0.25
  end

  # The share of the RING each slice was laid out in — read off where the next one starts, which is
  # the only place the span is stated. They close the ring by construction, so they always sum to 1.
  def arc_shares(arcs)
    starts = arcs.map { |arc| arc_start(arc) }
    (starts + [1.0]).each_cons(2).map { |from, to| to - from }
  end

  # How many of the slices were too small for an arc and came out as the two round caps meeting.
  def dots(arcs)
    arcs.count { |arc| arc[:dash].split.first.to_f.zero? }
  end

  # What survives the fold, as [percent, colour] — the composition, before any of it is drawn.
  def icon_shares(pairs)
    slices = ring_slices(pairs.map { |value, color| [value.to_d, color, nil] }, nil)
    total = slices.sum(0.to_d) { |value, _, _| value }
    slices.map { |value, color, _| [(value / total * 100).round(1).to_f, color] }
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
    assert_in_delta 0.75, arc_shares(arcs).first, 0.001
    assert_in_delta 0.25, arc_shares(arcs).last, 0.001
    # Clockwise from twelve o'clock, each span starting exactly where the one before it ended.
    assert_in_delta 0.0, arc_start(arcs.first), 0.001
    assert_in_delta 0.75, arc_start(arcs.last), 0.001
  end

  # The icon IS the page's ring at 24px, so it is drawn under the page's own switch. A menu that
  # keeps a stablecoin slice the page below it has dropped is a second opinion about the portfolio,
  # from the one place a reader cannot check it against anything.
  test 'the icon drops the cash the tracker drops, and keeps it when the tracker keeps it' do
    user = create(:user)
    icon_balance(user, '#F7931A', 75)
    AccountBalance.create!(user: user, exchange: (@icon_exchange ||= create(:binance_exchange)),
                           asset: create(:asset, :usdt, color: '#26A17B'), free: 25, locked: 0,
                           usd_price: 1, usd_value: 25, synced_at: Time.current)

    assert_equal 1, allocation_icon_arcs(user).size, 'cash is not a position anybody picked'

    user.update!(tracker_settings: { 'show_cash' => true })

    assert_equal 2, allocation_icon_arcs(user).size
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
  # The pieces are the card's pieces. How many of them a 24px ring has room for is the layout's
  # business: they are placed largest first, each in the span its share deserves and never under the
  # smallest slice, until the ring is full — then one more, and they all give up the same fraction of
  # themselves to fit it.

  test 'a holding too small to draw at its own size is given the smallest slice' do
    arcs = icon_arcs([[96, '#4C6B4C'], [4, '#1652F0']].map { |value, color| [value.to_d, color] })

    assert_equal 2, arcs.size
    # No dash left at all: what is drawn is the two round caps meeting, a dot as wide as the stroke.
    assert_equal '0.0', arcs.last[:dash].split.first
    # It cost the holding above it a twentieth of its arc, and the ring still closes.
    assert_in_delta 0.069, arc_shares(arcs).last, 0.002
    assert_in_delta 1.0, arc_shares(arcs).sum, 0.001
  end

  # The dots are what run the ring out of room, and the holdings behind them are not drawn. What is
  # drawn keeps the share it really has — the space the dots borrow is exactly the space the pieces
  # that never got placed gave up, so no arc has to be inflated to close the ring.
  test 'the ring stops where it runs out, and the arcs keep their true share' do
    arcs = icon_arcs(([[80, '#4C6B4C']] + Array.new(6) { [3.33, '#1652F0'] })
                     .map { |value, color| [value.to_d, color] })
    shares = arc_shares(arcs)

    assert_equal 4, arcs.size, 'three dots is all the ring has room for beside an 80% holding'
    assert_in_delta 0.80, shares.first, 0.01
    assert_equal 3, dots(arcs)
    assert_in_delta 1.0, shares.sum, 0.001
  end

  # A ring with nothing to say about size says it by being full: every piece is under the smallest
  # slice, so every piece is a dot, and one more is placed than fits so the ring reads as truncated.
  test 'more holdings than the ring can hold: the smallest are not drawn' do
    arcs = icon_arcs(Array.new(40) { [2.5.to_d, '#F7931A'] })

    assert_equal 15, arcs.size
    assert_equal 15, dots(arcs)
    assert_in_delta 1.0, arc_shares(arcs).sum, 0.001
  end

  # A holding can be so much smaller than the one above it that the big one's share rounds to the
  # whole ring. The next piece still gets placed — the ring is judged full before a piece is laid
  # down, never after, so no piece is ever squeezed out by the rounding of the one before it.
  test 'a holding too small to round keeps its dot' do
    arcs = icon_arcs([[1_000_000_000_000, '#4C6B4C'], [0.00000001, '#1652F0']]
                     .map { |value, color| [value.to_d, color] })

    assert_equal 2, arcs.size
    assert_equal 1, dots(arcs)
    assert_in_delta 1.0, arc_shares(arcs).sum, 0.001
  end

  # == what is gathered ==
  #
  # The fold is the card's, exactly: the icon is that ring at 24px, and a menu that gathers a
  # different tail from the page below it is a second opinion about the portfolio, from the one
  # place a reader cannot hold it against anything.
  test 'only the tail is gathered, into one neutral slice, last' do
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

  test 'a tail of dust is still gathered, and still drawn' do
    assert_equal [[99.5, '#F7931A'], [0.5, TrackerHelper::NEUTRAL_COLOR]],
                 icon_shares([[99.5, '#F7931A'], [0.25, '#627EEA'], [0.25, '#26A17B']])
  end

  # One holding is not an "other": there is nothing to gather it with, and grey says a name is being
  # withheld, so it keeps its own.
  test 'a single holding below the threshold keeps its own colour' do
    assert_equal [[60.0, '#F7931A'], [39.0, '#627EEA'], [1.0, '#26A17B']],
                 icon_shares([[60, '#F7931A'], [39, '#627EEA'], [1, '#26A17B']])
  end

  # With nothing above the threshold there is nothing for a tail to be "other" than — the whole
  # portfolio is the tail, and one neutral ring is what the card draws for it too.
  test 'with no holding above the threshold, the whole ring is the remainder' do
    assert_equal [[100.0, TrackerHelper::NEUTRAL_COLOR]], icon_shares(Array.new(200) { [0.5, '#F7931A'] })
  end

  test 'every slice keeps its whole share: the spans meet, and they close the ring' do
    arcs = icon_arcs([[60, '#F7931A'], [25, '#627EEA'], [15, '#26A17B']].map { |value, color| [value.to_d, color] })

    assert_in_delta 1.0, arc_shares(arcs).sum, 0.001
    assert_in_delta 0.6, arc_start(arcs[1]), 0.001
    assert_in_delta 0.85, arc_start(arcs[2]), 0.001
  end

  # == the card's ring is the same ring, drawn large ==
  #
  # Same code, so the same guarantee: a slice too small for its own arc is still given a gap and a
  # round cap — drawn as a dot — rather than nothing, whatever the stroke is.
  def ring_holding(symbol, color, value)
    Holding.new(Asset.new(symbol: symbol, color: color), value.to_d)
  end

  # Grey says a name is being withheld, so it has to be withholding more than one.
  test 'one holding under the threshold is itself, not an "other"' do
    arcs = holdings_ring_arcs([ring_holding('BTC', '#F7931A', 99), ring_holding('POL', '#8247E5', 1)])

    assert_equal(%w[BTC POL], arcs.map { |arc| arc[:label] })
    assert_equal ensure_contrast('#8247E5'), arcs.last[:color]
    # And it is drawn all the same: too small for an arc of its own, so it is given the smallest
    # slice — the two round caps meeting, a dot as wide as the stroke.
    assert_equal '0.0', arcs.last[:dash].split.first
  end

  test 'the card ring folds the tail and still draws it, at the smallest slice' do
    arcs = holdings_ring_arcs([ring_holding('BTC', '#F7931A', 99), ring_holding('POL', '#8247E5', 0.5),
                               ring_holding('DUST', '#627EEA', 0.5)])

    assert_equal(['BTC', I18n.t('tracker.other')], arcs.map { |arc| arc[:label] })
    assert_equal ensure_contrast(TrackerHelper::NEUTRAL_COLOR), arcs.last[:color]
    # Nothing left to draw but the two round caps meeting: a dot as wide as the stroke.
    assert_equal '0.0', arcs.last[:dash].split.first
    # The ring still closes, and the dot still has a gap either side of it. Making room for a piece
    # too small to pay for itself costs every piece the same fraction, so a squeezed slice comes out
    # just under the minimum — here the gap gives up a fiftieth of its width.
    starts = arcs.map { |arc| -arc[:offset] - (TrackerHelper::RING_MIN_SPAN / 2) }
    spans = (starts + [TrackerHelper::RING_CIRCUMFERENCE]).each_cons(2).map { |from, to| to - from }

    assert_in_delta 0.0, starts.first, 0.01
    assert_in_delta TrackerHelper::RING_CIRCUMFERENCE, spans.sum, 0.01
    spans.each { |span| assert_operator span, :>=, TrackerHelper::RING_STROKE + (TrackerHelper::RING_GAP * 0.97) }
  end
end
