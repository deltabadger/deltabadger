require 'test_helper'

# Everything that leaves or moves in an account lands where the page can see it: in money in (a
# withdrawal, at the basis it carried) or in realised (a sale, a cost, a currency's own move). What
# was booked nowhere is what the backstop reported as "these figures do not add up" — a broker's
# dollar fee, a deposit's fee, a BNB fee, the euro's rise between deposit and purchase.
class Tracker::RemainingLeaksTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @kraken = create(:kraken_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @key_kraken = create(:api_key, user: @user, exchange: @kraken)
    create(:asset, :bitcoin)
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

  def euro(day, usd)
    FxRate.create!(currency: 'USD', date: @day.call(day).to_date, rate: usd)
  end

  def scopes = Tracker::Ledger.send(:scopes, @user)

  # Money in plus what was banked is what is held at cost plus the cash at its basis.
  def assert_balanced(summary)
    assert_equal summary.total_invested_usd + summary.realised_pnl_usd,
                 summary.positions.sum(0.to_d, &:cost_usd) + summary.cash_basis.values.sum(0.to_d)
  end

  def assert_every_scope_balanced
    scopes.each_value { |summary| assert_balanced summary }
  end

  test 'a futures fill opens no position and moves no money' do
    tx(:buy, day: 1, base_currency: 'BTCUSDT', base_amount: 1, quote_currency: 'USDT', quote_amount: 50_000, tx_id: 'futures-1')

    summary = Tracker::Ledger.for(@user)

    assert_empty summary.positions
    assert_equal 0.to_d, summary.total_invested_usd
    assert_empty summary.openings
    assert_every_scope_balanced
  end

  test 'futures income is the futures wallet\'s, not money in' do
    tx(:other_income, day: 1, base_currency: 'USDT', base_amount: 100, tx_id: 'usdt-futures-7')
    tx(:other_income, day: 1, base_currency: 'BTC', base_amount: '0.01'.to_d, tx_id: 'coin-futures-8')

    summary = Tracker::Ledger.for(@user)

    assert_equal 0.to_d, summary.total_invested_usd
    assert_empty summary.positions
    assert_every_scope_balanced
  end

  test 'margin interest and a margin liquidation leave the spot lots alone' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    tx(:fee, day: 2, base_currency: 'BTC', base_amount: '0.01'.to_d, tx_id: 'margin-interest-1-BTC')
    tx(:sell, day: 3, base_currency: 'BTC', base_amount: '0.5'.to_d, quote_currency: 'USDT', quote_amount: 400, tx_id: 'liquidation-9')

    summary = Tracker::Ledger.for(@user)

    assert_equal 1.to_d, summary.positions.sole.quantity
    assert_equal 0.to_d, summary.realised_pnl_usd
    assert_every_scope_balanced
  end

  test 'a borrowed row is nothing pending' do
    tx(:buy, day: 5, base_currency: 'BTCUSDT', base_amount: 1, quote_currency: 'USDT', quote_amount: 50_000, tx_id: 'futures-2')

    assert_empty Tracker::Figures.moved_since(AccountTransaction.for_user(@user), { @binance.id => @day.call(1) })
  end

  test 'euro converted to dollars in two legs realises what the dollars came to' do
    euro(1, '1.10'.to_d)
    tx(:deposit, day: 1, key: @key_kraken, base_currency: 'EUR', base_amount: 100)
    euro(2, '1.20'.to_d)
    tx(:sell, day: 2, key: @key_kraken, base_currency: 'EUR', base_amount: 100, quote_currency: nil, quote_amount: nil, group_id: 'T1')
    tx(:buy, day: 2, key: @key_kraken, base_currency: 'USD', base_amount: 125, quote_currency: nil, quote_amount: nil, group_id: 'T1')

    summary = Tracker::Ledger.for(@user)

    assert_equal 15.to_d, summary.realised_pnl_usd
    assert_equal({ 'USD' => 125.to_d }, summary.cash_basis)
    assert_every_scope_balanced
  end

  test 'a two-leg conversion\'s fee netted from the dollars is a cost' do
    euro(1, 1.to_d)
    tx(:deposit, day: 1, key: @key_kraken, base_currency: 'EUR', base_amount: 100)
    tx(:sell, day: 2, key: @key_kraken, base_currency: 'EUR', base_amount: 100, quote_currency: nil, quote_amount: nil, group_id: 'T3')
    tx(:buy, day: 2, key: @key_kraken, base_currency: 'USD', base_amount: 125, quote_currency: nil, quote_amount: nil,
             fee_currency: 'USD', fee_amount: 5, group_id: 'T3')

    summary = Tracker::Ledger.for(@user)

    assert_equal 20.to_d, summary.realised_pnl_usd, '25 on the conversion, 5 lost to the fee'
    assert_equal({ 'USD' => 120.to_d }, summary.cash_basis)
    assert_every_scope_balanced
  end

  test 'a two-leg conversion whose fee took all the dollars lost what the euro carried' do
    euro(1, 1.to_d)
    tx(:deposit, day: 1, key: @key_kraken, base_currency: 'EUR', base_amount: 5)
    tx(:sell, day: 2, key: @key_kraken, base_currency: 'EUR', base_amount: 5, quote_currency: nil, quote_amount: nil, group_id: 'T4')
    tx(:buy, day: 2, key: @key_kraken, base_currency: 'USD', base_amount: 5, quote_currency: nil, quote_amount: nil,
             fee_currency: 'USD', fee_amount: 10, group_id: 'T4')

    summary = Tracker::Ledger.for(@user)

    assert_equal(-5.to_d, summary.realised_pnl_usd)
    assert_empty summary.cash_basis
    assert_every_scope_balanced
  end

  test 'a two-leg conversion\'s fee on top of the euro is a cost' do
    euro(1, 1.to_d)
    tx(:deposit, day: 1, key: @key_kraken, base_currency: 'EUR', base_amount: 101)
    tx(:sell, day: 2, key: @key_kraken, base_currency: 'EUR', base_amount: 100, quote_currency: nil, quote_amount: nil,
              fee_currency: 'EUR', fee_amount: 1, group_id: 'T2')
    tx(:buy, day: 2, key: @key_kraken, base_currency: 'USD', base_amount: 110, quote_currency: nil, quote_amount: nil, group_id: 'T2')

    summary = Tracker::Ledger.for(@user)

    assert_equal 9.to_d, summary.realised_pnl_usd, '10 on the conversion, 1 lost to the fee'
    assert_every_scope_balanced
  end

  # ── a fee in a third coin FIFO never applied ─────────────────────────────────────────────

  def bnb_worth_five
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 5)
    tx(:buy, day: 1, base_currency: 'BNB', base_amount: 1, quote_currency: 'USDC', quote_amount: 5)
  end

  test 'a BNB fee on a withdrawal leaves the BNB, at its basis' do
    bnb_worth_five
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    tx(:withdrawal, day: 2, base_currency: 'BTC', base_amount: 1, fee_currency: 'BNB', fee_amount: 1)

    summary = Tracker::Ledger.for(@user)

    assert_empty summary.positions
    assert_equal(-5.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  test 'a BNB fee on a euro deposit leaves the BNB, at its basis' do
    bnb_worth_five
    euro(2, '1.10'.to_d)
    tx(:deposit, day: 2, base_currency: 'EUR', base_amount: 100, fee_currency: 'BNB', fee_amount: 1)

    summary = Tracker::Ledger.for(@user)

    assert_empty summary.positions
    assert_equal(-5.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  # Two BNB, one paid: exactly one must remain — a second take would find the other.
  test 'a BNB fee FIFO already took on a purchase is not taken twice' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_010)
    tx(:buy, day: 1, base_currency: 'BNB', base_amount: 2, quote_currency: 'USDC', quote_amount: 10)
    price('BNB', 2, 10)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000, fee_currency: 'BNB', fee_amount: 1)

    summary = Tracker::Ledger.for(@user)

    bnb = summary.positions.find { |position| position.symbol == 'BNB' }
    assert_equal 1.to_d, bnb.quantity
    assert_equal 5.to_d, bnb.cost_usd
    assert_equal 5.to_d, summary.realised_pnl_usd
    assert_every_scope_balanced
  end

  def bnb_sale(followed:)
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_010)
    tx(:buy, day: 1, base_currency: 'BNB', base_amount: 2, quote_currency: 'USDC', quote_amount: 10)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    price('BNB', 2, 10)
    tx(:sell, day: 2, base_currency: 'BTC', base_amount: '0.5'.to_d, quote_currency: 'USDC', quote_amount: 500,
              fee_currency: 'BNB', fee_amount: 1)
    tx(:deposit, day: 3, key: @key_kraken, base_currency: 'USD', base_amount: 1) if followed
  end

  test 'a BNB fee FIFO already took on a sale is not taken twice, on the last row' do
    bnb_sale(followed: false)

    assert_equal 1.to_d, scopes[@binance.id].positions.find { |position| position.symbol == 'BNB' }.quantity
    assert_every_scope_balanced
  end

  test 'a BNB fee FIFO already took on a sale is not taken twice, before another venue\'s row' do
    bnb_sale(followed: true)

    assert_equal 1.to_d, scopes[@binance.id].positions.find { |position| position.symbol == 'BNB' }.quantity
    assert_every_scope_balanced
  end

  test 'a BNB fee on a loss still leaves the BNB' do
    bnb_worth_five
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    tx(:lost, day: 2, base_currency: 'BTC', base_amount: 1, fee_currency: 'BNB', fee_amount: 1)

    summary = Tracker::Ledger.for(@user)

    assert_empty summary.positions
    assert_equal(-1_005.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  test 'a coin bought whose fee in the same coin took all of it lost what it cost' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000, fee_currency: 'BTC', fee_amount: 1)

    summary = Tracker::Ledger.for(@user)

    assert_empty summary.positions
    assert_equal(-1_000.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  # ── a coin sold with its fee in the coin sold ─────────────────────────────────────────────

  # Kraken and Binance both take it on top: the balance falls by what was sold AND the fee.
  test 'a coin sold with its fee in the same coin pays the fee on top, at its basis' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    price('BTC', 3, 2_000)
    tx(:sell, day: 3, base_currency: 'BTC', base_amount: '0.5'.to_d, quote_currency: 'USDC', quote_amount: 1_000,
              fee_currency: 'BTC', fee_amount: '0.01'.to_d)

    summary = Tracker::Ledger.for(@user)

    assert_equal '0.49'.to_d, summary.positions.sole.quantity
    assert_equal 490.to_d, summary.positions.sole.cost_usd
    assert_equal 490.to_d, summary.realised_pnl_usd, 'sold half for 1,000 against 500, and 10 of cost went on the fee'
    assert_every_scope_balanced
  end

  test 'a coin swapped for a coin with its fee in the coin swapped away pays it on top, as a cost' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_010)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: '1.01'.to_d, quote_currency: 'USDC', quote_amount: 1_010)
    price('BTC', 2, 1_000)
    price('ETH', 2, 1_000)
    tx(:swap_out, day: 2, base_currency: 'BTC', base_amount: 1, fee_currency: 'BTC', fee_amount: '0.01'.to_d, group_id: 'S1')
    tx(:swap_in, day: 2, base_currency: 'ETH', base_amount: 1, group_id: 'S1')

    summary = Tracker::Ledger.for(@user)

    assert_equal %w[ETH], summary.positions.map(&:symbol)
    assert_equal(-10.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  test 'a coin swapped out to nowhere with a fee in the same coin pays it on top' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_010)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: '1.01'.to_d, quote_currency: 'USDC', quote_amount: 1_010)
    tx(:swap_out, day: 2, base_currency: 'BTC', base_amount: 1, fee_currency: 'BTC', fee_amount: '0.01'.to_d)

    summary = Tracker::Ledger.for(@user)

    assert_empty summary.positions
    assert_equal 10.to_d, summary.total_invested_usd, 'the coin left at its cost; the fee did not'
    assert_equal(-10.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  test 'what a sale moves is what was sold and its fee' do
    moves = Tracker::Ledger.quantity_moves({ entry_type: 'sell', base_currency: 'BTC', base_amount: '0.5'.to_d,
                                             fee_currency: 'BTC', fee_amount: '0.01'.to_d })

    assert_equal [['BTC', -'0.51'.to_d]], moves
  end
end
