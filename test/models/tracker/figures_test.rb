require 'test_helper'

# The page's figures, from one place — and the assumptions behind them, in words beside them.
#
# The venue's balance is the truth about what is held; the history explains what it can. Where the
# two do not meet, `Tracker::Figures` fills the gap with the most reasonable assumption and notes
# it — see `resolution_test.rb` for every rule. What is tested here is the frame around them: the
# identity when nothing needs assuming, dust, the backstop, and what "since the sync" means.
class Tracker::FiguresTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @kraken = create(:kraken_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @key_kraken = create(:api_key, user: @user, exchange: @kraken)
    @btc = create(:asset, :bitcoin)
    @day = ->(n) { Time.utc(2026, 1, n, 12) }
  end

  def tx(type, day:, key: @key, **attrs)
    defaults = { api_key: key, exchange: key.exchange, entry_type: type, transacted_at: @day.call(day) }
    defaults.merge!(quote_currency: nil, quote_amount: nil) unless %i[buy sell].include?(type)
    create(:account_transaction, **defaults, **attrs)
  end

  def balance(asset, quantity, value)
    AccountBalance.create!(user: @user, exchange: @binance, asset: asset, free: quantity, locked: 0,
                           usd_price: quantity.zero? ? 0 : value / quantity, usd_value: value,
                           synced_at: Time.current, priced_at: Time.current)
  end

  def figures(pending: {})
    Tracker::Figures.for(@user, ledger: Tracker::Ledger.for(@user),
                                balances: AccountBalance.for_user(@user).nonzero.includes(:asset).to_a,
                                pending: pending)
  end

  test 'when everything agrees, every figure is stated, nothing is noted, and the identity holds' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, 1, 1_500)

    result = figures

    assert_empty result.notes
    assert_equal 1_000.to_d, result.invested
    assert_equal 1_500.to_d, result.value
    assert_equal 500.to_d, result.unrealised
    assert_equal 500.to_d, result.total
    assert_equal result.value - result.invested, result.realised + result.unrealised
  end

  # Exchanges and histories never agree to the last digit: dust is assumed away in silence.
  test 'a rounding difference is applied in silence' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, 0.9999, 1_500)

    assert_empty figures.notes
  end

  test 'cash is not a position: no cost, no gain, no note' do
    usdc = create(:asset, symbol: 'USDC', name: 'USD Coin')
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    balance(usdc, 1_000, 1_000)

    result = figures

    assert_empty result.notes
    assert_equal 1_000.to_d, result.value
    assert_nil result.holdings.sole.unrealised
  end

  # ── since the sync ───────────────────────────────────────────────────────────────────────
  test 'cash moved since the sync in another currency is brought forward at today\'s rate' do
    eur = create(:asset, symbol: 'EUR', name: 'Euro')
    [@day.call(1).to_date, @day.call(4).to_date, Date.current].each do |date|
      FxRate.create!(currency: 'USD', date: date, rate: 1.25)
    end
    tx(:deposit, day: 1, base_currency: 'EUR', base_amount: 1_000)
    tx(:buy, day: 4, base_currency: 'BTC', base_amount: 0.1, quote_currency: 'EUR', quote_amount: 400)
    balance(eur, 1_000, 1_250)
    pending = Tracker::Figures.moved_since(AccountTransaction.for_user(@user), { @binance.id => @day.call(3) })

    result = figures(pending: pending)

    assert_equal(-400.to_d, pending['EUR'])
    assert_empty(result.notes.select { |note| note.kind.to_s.start_with?('cash') }, '600 EUR left, at 1.25: 750, as the ledger has it')
  end

  test 'cash moved since the sync is read as the ledger reads it: net of its own fee, and never borrowed' do
    tx(:deposit, day: 4, base_currency: 'USDC', base_amount: 1_000, fee_currency: 'USDC', fee_amount: 5)
    tx(:buy, day: 5, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDT', quote_amount: 100, tx_id: 'futures-1')

    pending = Tracker::Figures.moved_since(AccountTransaction.for_user(@user), { @binance.id => @day.call(3) })

    assert_equal({ 'USDC' => 995.to_d, 'BTC' => 1.to_d }, pending)
  end

  test 'a venue with no watermark is not brought forward' do
    tx(:buy, day: 4, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 100)
    tx(:buy, day: 4, key: @key_kraken, base_currency: 'ETH', base_amount: 1, quote_currency: 'USDC', quote_amount: 100)

    pending = Tracker::Figures.moved_since(AccountTransaction.for_user(@user), { @binance.id => @day.call(3), @kraken.id => nil })

    assert_equal({ 'BTC' => 1.to_d, 'USDC' => -100.to_d }, pending)
  end

  test 'cash a sale created since the sync is held, not moved out' do
    create(:asset, symbol: 'USDC', name: 'USD Coin')
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    tx(:sell, day: 4, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_500)
    balance(@btc, 1, 1_200) # taken on day 3: the coin, and no USDC row at all
    pending = Tracker::Figures.moved_since(AccountTransaction.for_user(@user), { @binance.id => @day.call(3) })

    result = figures(pending: pending)

    assert_empty result.notes
    assert_equal 'USDC', result.holdings.sole.asset.symbol
    assert_equal 1_500.to_d, result.value
    assert_equal 1_000.to_d, result.invested
    assert_equal 500.to_d, result.realised
  end

  test 'quantities moved since the sync are read as the engine reads them: net of a fee in kind; a fee in a third asset leaves it' do
    tx(:buy, day: 4, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 100, fee_currency: 'BTC', fee_amount: 0.01)
    tx(:buy, day: 4, base_currency: 'ETH', base_amount: 1, quote_currency: 'USDC', quote_amount: 100, fee_currency: 'BNB', fee_amount: 0.1)

    pending = Tracker::Figures.moved_since(AccountTransaction.for_user(@user), { @binance.id => @day.call(3) })

    assert_equal({ 'BTC' => 0.99.to_d, 'ETH' => 1.to_d, 'BNB' => -0.1.to_d, 'USDC' => -200.to_d }, pending)
  end

  test 'a transfer is linked for what is pending only when both legs are: a leg whose far end the balances already hold moves whole' do
    deposit = tx(:deposit, day: 4, key: @key_kraken, base_currency: 'BTC', base_amount: 0.99)
    tx(:withdrawal, day: 4, base_currency: 'BTC', base_amount: 1, linked_transaction: deposit)
    scope = AccountTransaction.for_user(@user)

    both = Tracker::Figures.moved_since(scope, { @binance.id => @day.call(3), @kraken.id => @day.call(3) })
    source_only = Tracker::Figures.moved_since(scope, { @binance.id => @day.call(3), @kraken.id => @day.call(5) })
    destination_only = Tracker::Figures.moved_since(scope, { @binance.id => @day.call(5), @kraken.id => @day.call(3) })

    assert_equal({ 'BTC' => -0.01.to_d }, both, 'the network fee, as the ledger has it')
    assert_equal({ 'BTC' => -1.to_d }, source_only, 'Kraken already holds the 0.99: the whole coin left Binance')
    assert_equal({ 'BTC' => 0.99.to_d }, destination_only, 'Binance already lacks the coin: the 0.99 arrived')
  end

  test 'a reverse split since the sync keeps its sign' do
    tx(:adjustment, day: 4, base_currency: 'AAPL', base_amount: -50)

    pending = Tracker::Figures.moved_since(AccountTransaction.for_user(@user), { @binance.id => @day.call(3) })

    assert_equal({ 'AAPL' => -50.to_d }, pending)
  end

  # ── the backstop ─────────────────────────────────────────────────────────────────────────
  #
  # Every assumption moves money in and basis together, so what is held less what went in is what
  # was banked plus what is still riding — by construction. When it is not, something is wrong that
  # no assumption anticipated, and the page says so rather than showing figures that quietly
  # contradict each other. It runs whatever else was noted.
  test 'figures that do not add up say so, even beside another note' do
    broken = Tracker::Ledger::Summary.new(
      positions: [Tracker::Ledger::Position.new(symbol: 'BTC', asset: @btc, quantity: 1.to_d, cost_usd: 1_000.to_d,
                                                avg_cost_usd: 1_000.to_d, opened_at: @day.call(1), estimated: false,
                                                unpriced_quantity: 0.to_d)],
      round_trips: [], total_invested_usd: 500.to_d, received_usd: 0.to_d, realised_pnl_usd: 0.to_d,
      fees_usd: 0.to_d, cash_usd: 0.to_d, cash: {}, unpriced_proceeds_usd: 0.to_d, incomplete: false, openings: {},
      computed_at: Time.current
    )
    balance(@btc, 0.4, 600) # the venue holds less: the rest left at cost, 600...

    result = Tracker::Figures.for(@user, ledger: broken, balances: AccountBalance.for_user(@user).includes(:asset).to_a)

    # ...and the ledger disagrees with itself: 1,000 of cost against 500 of money in and nothing realised.
    assert_equal %i[figures_disagree left], result.notes.map(&:kind).sort
    assert_equal 500.to_d, result.notes.find { |note| note.kind == :figures_disagree }.amount_usd
  end

  # The one operation "Show cash: off" performs: the cash holdings leave the LIST, and nothing else
  # moves. Every figure is still the whole portfolio — hiding a balance is not an accounting choice
  # — and doing it here rather than in each template is what keeps the card, the ring, the type
  # shares and the positions table reading from one list.
  test 'without_cash drops the cash holdings and leaves every figure where it was' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 600)
    balance(@btc, 1, 900)
    balance(create(:asset, symbol: 'USDC', name: 'USD Coin', external_id: 'usd-coin'), 400, 400)

    full = figures
    lean = full.without_cash

    assert_equal %w[BTC USDC], full.holdings.map { |holding| holding.asset.symbol }.sort
    assert_equal(%w[BTC], lean.holdings.map { |holding| holding.asset.symbol })
    assert_equal full.to_h.except(:holdings), lean.to_h.except(:holdings)
    assert_equal 1_300.to_d, lean.value, 'the portfolio is still worth what it is worth'
  end

  # A portfolio with no cash in it is the same portfolio, and a ledger still warming has holdings
  # to filter before it has any figures at all.
  test 'without_cash is a no-op on a portfolio holding none, and safe while the ledger warms' do
    balance(@btc, 1, 900)

    assert_equal figures.holdings, figures.without_cash.holdings

    warming = Tracker::Figures.for(@user, ledger: nil, balances: AccountBalance.for_user(@user).includes(:asset).to_a)
    assert_nil warming.without_cash.invested
    assert_equal(%w[BTC], warming.without_cash.holdings.map { |holding| holding.asset.symbol })
  end
end
