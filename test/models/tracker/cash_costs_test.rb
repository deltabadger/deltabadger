require 'test_helper'

# Everything that leaves or moves in an account lands where the page can see it: in money in (a
# withdrawal, at the basis it carried) or in realised (a sale, a cost, a currency's own move). What
# was booked nowhere is what the backstop reported as "these figures do not add up" — a broker's
# dollar fee, a deposit's fee, a BNB fee, the euro's rise between deposit and purchase.
class Tracker::CashCostsTest < ActiveSupport::TestCase
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

  # ── costs ──────────────────────────────────────────────────────────────────────────────

  test 'a broker fee paid in dollars is a realised loss' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 1_000)
    tx(:fee, day: 2, base_currency: 'USD', base_amount: '8.58'.to_d)

    summary = Tracker::Ledger.for(@user)

    assert_equal 1_000.to_d, summary.total_invested_usd
    assert_equal(-'8.58'.to_d, summary.realised_pnl_usd)
    assert_equal({ 'USD' => '991.42'.to_d }, summary.cash_basis)
    assert_every_scope_balanced
  end

  test 'a fee netted out of a cash deposit is a realised loss' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: '1020.44'.to_d, fee_currency: 'USD', fee_amount: 3)

    summary = Tracker::Ledger.for(@user)

    assert_equal '1020.44'.to_d, summary.total_invested_usd
    assert_equal(-3.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  # `moves` clamps an arrival at zero: a fee larger than the deposit costs the deposit, no more.
  test 'a fee larger than its deposit costs only the deposit' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 5, fee_currency: 'USDC', fee_amount: 10)

    summary = Tracker::Ledger.for(@user)

    assert_equal(-5.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  test 'withholding tax taken in cash is a realised loss' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 100)
    tx(:withholding_tax, day: 2, base_currency: 'USD', base_amount: '1.5'.to_d)

    assert_equal(-'1.5'.to_d, Tracker::Ledger.for(@user).realised_pnl_usd)
    assert_every_scope_balanced
  end

  test 'euro lost is a realised loss at the basis it carried, not at the day it went' do
    euro(1, '1.10'.to_d)
    tx(:deposit, day: 1, base_currency: 'EUR', base_amount: 100)
    euro(3, '1.20'.to_d)
    tx(:lost, day: 3, base_currency: 'EUR', base_amount: 100)

    summary = Tracker::Ledger.for(@user)

    assert_equal 110.to_d, summary.total_invested_usd
    assert_equal(-110.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  test 'a stablecoin lost is realised once' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 100)
    tx(:lost, day: 2, base_currency: 'USDC', base_amount: 100)

    assert_equal(-100.to_d, Tracker::Ledger.for(@user).realised_pnl_usd)
    assert_every_scope_balanced
  end

  # No stablecoin lot exists: the dollars came from a sale's quote leg. A sale at zero out of no
  # lots would realise nothing; the book realises the face.
  test 'a stablecoin lost out of sale proceeds is realised once' do
    price('BTC', 1, 1_000)
    tx(:deposit, day: 1, base_currency: 'BTC', base_amount: 1)
    tx(:sell, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    tx(:lost, day: 3, base_currency: 'USDC', base_amount: 1_000)

    summary = Tracker::Ledger.for(@user)

    assert_equal(-1_000.to_d, summary.realised_pnl_usd, 'sold at cost, then the dollars were lost')
    assert_every_scope_balanced
  end

  # FIFO already puts a cash fee on a coin acquisition into the lot's cost: it is paid, not lost.
  test 'a cash fee on a coin deposit is in the lot, once' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 10)
    price('BTC', 2, 100)
    tx(:deposit, day: 2, base_currency: 'BTC', base_amount: 1, fee_currency: 'USD', fee_amount: 10)

    summary = Tracker::Ledger.for(@user)

    assert_equal 110.to_d, summary.positions.sole.cost_usd
    assert_equal 0.to_d, summary.realised_pnl_usd
    assert_every_scope_balanced
  end

  test 'a cash fee on a coin withdrawal is a realised loss' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_010)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    tx(:withdrawal, day: 2, base_currency: 'BTC', base_amount: 1, fee_currency: 'USDC', fee_amount: 10)

    summary = Tracker::Ledger.for(@user)

    assert_equal(-10.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  test 'a coin fee paid in kind is a realised loss at its basis' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    tx(:fee, day: 3, base_currency: 'BTC', base_amount: '0.1'.to_d)

    summary = Tracker::Ledger.for(@user)

    assert_equal 1_000.to_d, summary.total_invested_usd
    assert_equal(-100.to_d, summary.realised_pnl_usd)
    assert_equal 900.to_d, summary.positions.sole.cost_usd
    assert_every_scope_balanced
  end

  test 'the fee a coin transfer paid on the way is a realised loss' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    arrived = tx(:deposit, key: @key_kraken, day: 3, base_currency: 'BTC', base_amount: '0.99'.to_d)
    tx(:withdrawal, day: 2, base_currency: 'BTC', base_amount: 1, linked_transaction: arrived)

    assert_equal(-10.to_d, scopes[nil].realised_pnl_usd)
    assert_equal 990.to_d, scopes[@kraken.id].positions.sole.cost_usd
    assert_every_scope_balanced
  end

  # ── a fee paid in a third coin ─────────────────────────────────────────────────────────

  # BNB bought at 5 pays a fee worth 10: the BTC lot carries the 10, BNB gives up its 5, and the 5
  # between is a gain on the BNB — a disposal FIFO never recorded.
  test 'a fee paid in BNB realises what the BNB gained' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_005)
    tx(:buy, day: 1, base_currency: 'BNB', base_amount: 1, quote_currency: 'USDC', quote_amount: 5)
    price('BNB', 2, 10)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000,
             fee_currency: 'BNB', fee_amount: 1)

    summary = Tracker::Ledger.for(@user)

    assert_equal 1_010.to_d, summary.positions.sole.cost_usd
    assert_equal 5.to_d, summary.realised_pnl_usd
    assert_every_scope_balanced
  end

  test 'a stablecoin fee from sale proceeds is paid once, and gains nothing' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 500)
    price('BTC', 1, 1_000)
    tx(:deposit, day: 1, base_currency: 'BTC', base_amount: 1)
    tx(:sell, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    tx(:buy, day: 3, base_currency: 'ETH', base_amount: 1, quote_currency: 'USD', quote_amount: 500,
             fee_currency: 'USDC', fee_amount: 10)

    summary = Tracker::Ledger.for(@user)

    assert_equal 510.to_d, summary.positions.sole.cost_usd
    assert_equal 0.to_d, summary.realised_pnl_usd
    assert_every_scope_balanced
  end

  # A coin-for-coin swap hands its lots on and reads no fee (`transfer_swap_out`): a cash fee on it
  # bought nothing.
  test 'a cash fee on a coin-for-coin swap is a realised loss' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 10)
    price('BTC', 1, 100)
    price('ETH', 2, 100)
    tx(:deposit, day: 1, base_currency: 'BTC', base_amount: 1)
    tx(:swap_out, day: 2, base_currency: 'BTC', base_amount: 1, group_id: 'swap-1', fee_currency: 'USD', fee_amount: 10)
    tx(:swap_in, day: 2, base_currency: 'ETH', base_amount: 1, group_id: 'swap-1')

    summary = Tracker::Ledger.for(@user)

    assert_equal 100.to_d, summary.positions.sole.cost_usd
    assert_equal(-10.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  # An arrival FIFO opens a lot for — orphan or not — takes its fee into that lot.
  test 'a cash fee on a coin arriving from nowhere is in its lot, once' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 10)
    price('ETH', 2, 100)
    tx(:swap_in, day: 2, base_currency: 'ETH', base_amount: 1, fee_currency: 'USD', fee_amount: 10)

    summary = Tracker::Ledger.for(@user)

    assert_equal 110.to_d, summary.positions.sole.cost_usd
    assert_equal 0.to_d, summary.realised_pnl_usd
    assert_every_scope_balanced
  end

  # The book owns cash: FIFO must not also sell stablecoin lots that never existed.
  test 'stablecoins sold for dollars realise nothing out of lots that were never there' do
    price('BTC', 1, 100)
    tx(:deposit, day: 1, base_currency: 'BTC', base_amount: 1)
    tx(:sell, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 100)
    tx(:sell, day: 3, base_currency: 'USDC', base_amount: 100, quote_currency: 'USD', quote_amount: 100)

    summary = Tracker::Ledger.for(@user)

    assert_equal 0.to_d, summary.realised_pnl_usd
    assert_equal({ 'USD' => 100.to_d }, summary.cash_basis)
    assert_every_scope_balanced
  end

  # As a coin bought with its own fee: the fee shrinks what arrived, the cost stays what was paid.
  test 'euro bought with dollars keeps what it cost, fee and all' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 125)
    euro(2, '1.20'.to_d)
    tx(:buy, day: 2, base_currency: 'EUR', base_amount: 100, quote_currency: 'USD', quote_amount: 125,
             fee_currency: 'EUR', fee_amount: 2)

    summary = Tracker::Ledger.for(@user)

    assert_equal 0.to_d, summary.realised_pnl_usd
    assert_equal({ 'EUR' => 125.to_d }, summary.cash_basis)
    assert_every_scope_balanced
  end

  # Grouped legs settled against cash carry the quote currency and no amount; FIFO still disposes.
  test 'a cash fee on a coin sold for stablecoins in two legs comes off once' do
    price('BTC', 1, 100)
    tx(:deposit, day: 1, base_currency: 'BTC', base_amount: 1)
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 10)
    tx(:swap_out, day: 2, base_currency: 'BTC', base_amount: 1, group_id: 'conv-1', fee_currency: 'USD', fee_amount: 10)
    tx(:swap_in, day: 2, base_currency: 'USDC', base_amount: 100, group_id: 'conv-1')

    summary = Tracker::Ledger.for(@user)

    assert_equal(-10.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  test 'a coin fee on buying stablecoins is a realised loss at its basis' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 1_005)
    tx(:buy, day: 1, base_currency: 'BNB', base_amount: 1, quote_currency: 'USD', quote_amount: 5)
    price('BNB', 2, 10)
    tx(:buy, day: 2, base_currency: 'USDC', base_amount: 1_000, quote_currency: 'USD', quote_amount: 1_000,
             fee_currency: 'BNB', fee_amount: 1)

    summary = Tracker::Ledger.for(@user)

    assert_equal(-5.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  # A settlement leg's own fee is charged on top (`FEE_ON_TOP`): the amount left, the fee was spent.
  test 'dollars swapped out to nowhere with a fee on top: the amount leaves, the fee is lost' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 110)
    tx(:swap_out, day: 2, base_currency: 'USD', base_amount: 100, fee_currency: 'USD', fee_amount: 10)

    summary = Tracker::Ledger.for(@user)

    assert_equal 10.to_d, summary.total_invested_usd
    assert_equal(-10.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  test 'a fee on top of dollars swapped out to nowhere is charged in full' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 15)
    tx(:swap_out, day: 2, base_currency: 'USD', base_amount: 5, fee_currency: 'USD', fee_amount: 10)

    summary = Tracker::Ledger.for(@user)

    assert_equal 10.to_d, summary.total_invested_usd
    assert_equal(-10.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  test 'a coin fee on selling stablecoins still leaves, as a cost at its basis' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_005)
    tx(:buy, day: 1, base_currency: 'BNB', base_amount: 1, quote_currency: 'USDC', quote_amount: 5)
    price('BNB', 2, 10)
    tx(:sell, day: 2, base_currency: 'USDC', base_amount: 1_000, quote_currency: 'USD', quote_amount: 1_000,
              fee_currency: 'BNB', fee_amount: 1)

    summary = Tracker::Ledger.for(@user)

    assert_empty summary.positions, 'the BNB paid the fee'
    assert_equal(-5.to_d, summary.realised_pnl_usd)
    assert_every_scope_balanced
  end

  test 'euro bought with dollars whose fee took all of it is a loss of what it cost' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 100)
    euro(2, '1.20'.to_d)
    tx(:buy, day: 2, base_currency: 'EUR', base_amount: 1, quote_currency: 'USD', quote_amount: 100,
             fee_currency: 'EUR', fee_amount: 1)

    summary = Tracker::Ledger.for(@user)

    assert_equal(-100.to_d, summary.realised_pnl_usd)
    assert_empty summary.cash_basis
    assert_every_scope_balanced
  end

  # ── currency ───────────────────────────────────────────────────────────────────────────

  test 'euro that rose before it was spent realises the rise' do
    euro(1, '1.10'.to_d)
    tx(:deposit, day: 1, base_currency: 'EUR', base_amount: 1_000)
    euro(5, '1.20'.to_d)
    tx(:buy, day: 5, base_currency: 'BTC', base_amount: 1, quote_currency: 'EUR', quote_amount: 1_000)

    summary = Tracker::Ledger.for(@user)

    assert_equal 1_100.to_d, summary.total_invested_usd
    assert_equal 1_200.to_d, summary.positions.sole.cost_usd
    assert_equal 100.to_d, summary.realised_pnl_usd
    assert_every_scope_balanced
  end

  test 'euro held keeps the basis it arrived at' do
    euro(1, '1.10'.to_d)
    tx(:deposit, day: 1, base_currency: 'EUR', base_amount: 1_000)
    euro(5, '1.20'.to_d)
    tx(:buy, day: 5, base_currency: 'BTC', base_amount: 1, quote_currency: 'EUR', quote_amount: 400)

    summary = Tracker::Ledger.for(@user)

    assert_equal({ 'EUR' => 660.to_d }, summary.cash_basis)
    assert_equal 40.to_d, summary.realised_pnl_usd, '400 euro bought 480 of basis and had cost 440'
    assert_every_scope_balanced
  end

  # As a coin leaving takes out what it cost, cash leaving takes out what it carried in.
  test 'euro withdrawn after it rose leaves at its basis' do
    euro(1, '1.10'.to_d)
    tx(:deposit, day: 1, base_currency: 'EUR', base_amount: 1_000)
    euro(5, '1.20'.to_d)
    tx(:withdrawal, day: 5, base_currency: 'EUR', base_amount: 1_000)

    summary = Tracker::Ledger.for(@user)

    assert_equal 0.to_d, summary.total_invested_usd
    assert_equal 0.to_d, summary.realised_pnl_usd
    assert_every_scope_balanced
  end

  test 'dollars swapped out to nowhere leave money in at their face' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 100)
    tx(:swap_out, day: 2, base_currency: 'USD', base_amount: 100)

    summary = Tracker::Ledger.for(@user)

    assert_equal 0.to_d, summary.total_invested_usd
    assert_every_scope_balanced
  end

  # A transfer between the user's own pots realises nothing, whatever the rate did on the way.
  test 'euro moved between venues carries its basis, at any rate' do
    euro(1, '1.10'.to_d)
    tx(:deposit, day: 1, base_currency: 'EUR', base_amount: 100)
    euro(2, '1.20'.to_d)
    euro(3, '1.30'.to_d)
    arrived = tx(:deposit, key: @key_kraken, day: 3, base_currency: 'EUR', base_amount: 98)
    tx(:withdrawal, day: 2, base_currency: 'EUR', base_amount: 100, linked_transaction: arrived)

    all = scopes[nil]

    assert_equal 110.to_d, all.total_invested_usd
    assert_equal(-'2.2'.to_d, all.realised_pnl_usd, 'the two euro the transfer cost, at their basis')
    assert_equal({ 'EUR' => '107.8'.to_d }, scopes[@kraken.id].cash_basis)
    assert_every_scope_balanced
  end

  # Sold for what the venue paid, not for what the ECB says the euro was worth that day.
  test 'euro sold for dollars realises what the dollars came to' do
    euro(1, '1.10'.to_d)
    tx(:deposit, day: 1, base_currency: 'EUR', base_amount: 100)
    euro(2, '1.20'.to_d)
    tx(:sell, day: 2, base_currency: 'EUR', base_amount: 100, quote_currency: 'USD', quote_amount: 125)

    summary = Tracker::Ledger.for(@user)

    assert_equal 15.to_d, summary.realised_pnl_usd
    assert_equal({ 'USD' => 125.to_d }, summary.cash_basis)
    assert_every_scope_balanced
  end

  # A venue that reports trades but not the transfer behind them realises nothing on the money it
  # had to infer: the shortfall arrives at the day's value and leaves at the same.
  test 'euro spent before it was seen arriving realises nothing' do
    euro(5, '1.20'.to_d)
    tx(:buy, day: 5, key: @key_kraken, base_currency: 'BTC', base_amount: 1, quote_currency: 'EUR', quote_amount: 1_000)

    summary = Tracker::Ledger.for(@user)

    assert_equal 1_200.to_d, summary.total_invested_usd
    assert_equal 0.to_d, summary.realised_pnl_usd
    assert_every_scope_balanced
  end
end
