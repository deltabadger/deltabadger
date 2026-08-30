require 'test_helper'

# The exchange's balance is the truth, the history explains what it can, and the page fills the
# rest with the most reasonable assumption — and says so, in gray, on the row it concerns. Nothing
# asks the user anything, nothing is written into the record, and no figure is ever blank.
#
# Two layers, because they need different inputs. The LEDGER (transactions only) opens each asset
# with what must have been held before its history begins — the smallest quantity that keeps the
# running balance at or above zero — at the market price of that day. FIGURES (balances + ledger)
# then resolves the final difference: what the history has beyond the balance left at cost, what
# the balance has beyond the history arrived at its price, cash the venue lacks moved out.
class Tracker::ResolutionTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @kraken = create(:kraken_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @key_kraken = create(:api_key, user: @user, exchange: @kraken)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @usdc = create(:asset, symbol: 'USDC', name: 'USD Coin')
    @day = ->(n) { Time.utc(2026, 1, n, 12) }
  end

  def tx(type, day:, key: @key, **attrs)
    defaults = { api_key: key, exchange: key.exchange, entry_type: type, transacted_at: @day.call(day) }
    defaults.merge!(quote_currency: nil, quote_amount: nil) unless %i[buy sell].include?(type)
    create(:account_transaction, **defaults, **attrs)
  end

  def price(symbol, day, usd)
    HistoricalPrice.create!(asset: symbol, currency: 'USD', date: @day.call(day).to_date, price: usd)
  end

  def balance(asset, quantity, value, exchange: @binance, synced_at: Time.current)
    AccountBalance.create!(user: @user, exchange: exchange, asset: asset, free: quantity, locked: 0,
                           usd_price: quantity.zero? ? 0 : value / quantity, usd_value: value,
                           synced_at: synced_at, priced_at: synced_at)
  end

  def figures(pending: {})
    Tracker::Figures.for(@user, ledger: Tracker::Ledger.for(@user),
                                balances: AccountBalance.for_user(@user).nonzero.includes(:asset).to_a,
                                pending: pending)
  end

  def note(result, kind) = result.notes.find { |n| n.kind == kind }

  # ── the ledger opens with what must have been held ─────────────────────────────────────────
  test 'a history that goes below zero opens with what must have been held, at that day\'s price' do
    price('BTC', 2, 20_000)
    tx(:sell, day: 2, base_currency: 'BTC', base_amount: 0.5, quote_currency: 'USDC', quote_amount: 12_000)

    summary = Tracker::Ledger.for(@user)

    assert_equal({ 'BTC' => 0.5.to_d }, summary.openings)
    assert_equal 10_000.to_d, summary.total_invested_usd, 'held before the history began, at 20,000'
    assert_equal 2_000.to_d, summary.realised_pnl_usd, 'sold for 12,000 against that basis, not against nothing'
    assert_not summary.incomplete, 'no disposal is uncovered any more'
    assert_empty summary.positions
  end

  test 'the opening balance covers every way coins leave' do
    price('BNB', 2, 100)
    tx(:buy, day: 2, base_currency: 'LTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 100,
             fee_currency: 'BNB', fee_amount: 0.01)
    tx(:withdrawal, day: 3, base_currency: 'BNB', base_amount: 0.02)

    assert_equal 0.03.to_d, Tracker::Ledger.for(@user).openings['BNB'], 'a fee paid in it and a withdrawal of it'
  end

  test 'an opening balance nobody can price is taken at zero cost, and says so' do
    tx(:sell, day: 2, base_currency: 'ZZZ', base_amount: 5, quote_currency: 'USDC', quote_amount: 50)

    summary = Tracker::Ledger.for(@user)

    assert_equal({ 'ZZZ' => 5.to_d }, summary.openings)
    assert_equal 0.to_d, summary.total_invested_usd
    assert_equal 50.to_d, summary.realised_pnl_usd, 'stated, on the assumption of zero cost'
    assert_equal 50.to_d, summary.unpriced_proceeds_usd, 'and the assumption is carried, for the note'
  end

  test 'the opening is history: money in carries it from its day' do
    price('BTC', 2, 20_000)
    tx(:sell, day: 2, base_currency: 'BTC', base_amount: 0.5, quote_currency: 'USDC', quote_amount: 12_000)

    first = Tracker::Ledger.money_in(@user).first

    assert_equal 10_000.to_d, first.amount
    assert_operator first.at, :<, @day.call(2)
  end

  # ── figures resolve the balance ────────────────────────────────────────────────────────────
  test 'history ahead of the balance: the extra left at cost' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, 0.9, 1_350)

    result = figures

    assert_equal 900.to_d, result.invested, '1,000 in, 100 of it left with the tenth that is gone'
    assert_equal 900.to_d, result.holdings.sole.cost
    assert_equal 450.to_d, result.unrealised
    assert_equal result.value - result.invested, result.realised + result.unrealised
    left = note(result, :left)
    assert_equal ['BTC', 'Binance', 1.to_d, 0.9.to_d, 100.to_d],
                 [left.symbol, left.exchange, left.history, left.held, left.amount_usd]
  end

  test 'a coin the venue no longer holds left at cost, and leaves the positions' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 600)
    balance(@usdc, 400, 400)

    result = figures

    assert_equal 400.to_d, result.invested
    assert_equal(%w[USDC], result.holdings.map { |h| h.asset.symbol })
    assert_equal 0.to_d, result.total
    assert_equal [1.to_d, 0.to_d, 600.to_d], note(result, :left).to_h.values_at(:history, :held, :amount_usd)
  end

  test 'the venue holds more than the history: the extra arrived at its price' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, 1.2, 1_800)

    result = figures

    assert_equal 1_300.to_d, result.invested, '1,000 paid and 0.2 more at 1,500, as if bought there'
    assert_equal 1_300.to_d, result.holdings.sole.cost
    assert_equal 500.to_d, result.unrealised
    assert_equal result.value - result.invested, result.realised + result.unrealised
    assert_equal [1.to_d, 1.2.to_d, 300.to_d], note(result, :arrived).to_h.values_at(:history, :held, :amount_usd)
  end

  test 'a coin with no history at all arrived at its price' do
    balance(@eth, 2, 200)

    result = figures

    assert_equal 200.to_d, result.invested
    assert_equal 0.to_d, result.unrealised
    assert_equal [0.to_d, 2.to_d, 200.to_d], note(result, :arrived).to_h.values_at(:history, :held, :amount_usd)
  end

  test 'cash the venue lacks moved out; cash it has beyond the history moved in' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    short = balance(@usdc, 700, 700)

    result = figures
    assert_equal 700.to_d, result.invested
    assert_equal 300.to_d, note(result, :cash_out).amount_usd

    short.update!(free: 1_200, usd_value: 1_200)
    result = figures
    assert_equal 1_200.to_d, result.invested
    assert_equal 200.to_d, note(result, :cash_in).amount_usd
  end

  # The identity — what is held less what went in is what was banked plus what is still riding —
  # holds by construction: every assumption moves money in and basis together.
  test 'the identity holds after every resolution at once' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 600)
    tx(:sell, day: 3, base_currency: 'BTC', base_amount: 0.5, quote_currency: 'USDC', quote_amount: 400)
    balance(@btc, 0.4, 320)   # the history has 0.5: a tenth left at cost, 60
    balance(@eth, 1, 100)     # no history: arrived at 100
    balance(@usdc, 750, 750)  # the history has 800: 50 moved out

    result = figures

    assert_equal 990.to_d, result.invested
    assert_equal 1_170.to_d, result.value
    assert_equal 100.to_d, result.realised
    assert_equal 80.to_d, result.unrealised
    assert_equal result.value - result.invested, result.realised + result.unrealised
    assert_equal %i[arrived cash_out left], result.notes.map(&:kind).sort
  end

  test 'below the floor the note is silent, and the arithmetic is not' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, '0.9995'.to_d, '1499.25'.to_d)

    result = figures

    assert_empty result.notes
    assert_in_delta 999.5, result.invested, 0.0001, 'a tenth of a cent left at cost — silently'
  end

  test 'a coin bought since the venue\'s last sync is held, at cost until the next sync' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_100)
    balance(@usdc, 1_100, 1_100, synced_at: @day.call(3))
    tx(:buy, day: 4, base_currency: 'ETH', base_amount: 1, quote_currency: 'USDC', quote_amount: 100)
    pending = Tracker::Figures.moved_since(AccountTransaction.for_user(@user), { @binance.id => @day.call(3) })

    result = figures(pending: pending)

    assert_equal({ 'ETH' => 1.to_d, 'USDC' => -100.to_d }, pending)
    eth = result.holdings.find { |h| h.asset.symbol == 'ETH' }
    assert_equal [1.to_d, 100.to_d, 0.to_d], [eth.quantity, eth.value, eth.unrealised]
    assert_equal 1_000.to_d, result.holdings.find { |h| h.asset.symbol == 'USDC' }.value, 'the cash it spent is gone from the snapshot too'
    assert_equal 1_100.to_d, result.invested
    assert_equal result.value - result.invested, result.realised + result.unrealised
    assert_equal 'ETH', note(result, :since_sync).symbol
  end

  test 'pending runs per exchange from its own watermark' do
    tx(:buy, day: 4, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 100)
    tx(:buy, day: 4, key: @key_kraken, base_currency: 'ETH', base_amount: 1, quote_currency: 'USDC', quote_amount: 100)

    pending = Tracker::Figures.moved_since(AccountTransaction.for_user(@user),
                                           { @binance.id => @day.call(5), @kraken.id => @day.call(3) })

    assert_equal({ 'ETH' => 1.to_d, 'USDC' => -100.to_d }, pending, 'Binance is up to date; Kraken is not')
  end

  test 'an unpriced lot is taken at zero cost, and says so' do
    tx(:airdrop, day: 1, base_currency: 'ZZZ', base_amount: 5)
    tx(:sell, day: 2, base_currency: 'ZZZ', base_amount: 5, quote_currency: 'USDC', quote_amount: 50)
    balance(@usdc, 50, 50)

    result = figures

    assert_equal 50.to_d, result.realised
    assert_equal 0.to_d, result.invested
    assert_equal 50.to_d, note(result, :unpriced).amount_usd, 'sold out of coins nobody could price'
    assert_equal result.value - result.invested, result.realised + result.unrealised
  end

  test 'today\'s chart point is the tile\'s figure' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, 0.9, 1_350)
    @key.update!(balances_synced_at: Time.current)

    assert_equal figures.invested, PortfolioSnapshot.today_row(@user)[:invested_usd]
  end
end
