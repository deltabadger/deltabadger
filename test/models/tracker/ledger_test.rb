require 'test_helper'

# The tracker's figure service: what the ledger alone can say about a portfolio — total invested
# (net money in), realised P/L, fees, open positions and closed round-trips — computed once per
# sync and cached, never in a request. FIFO lots come from the tax engine; nothing is walked twice.
class Tracker::LedgerTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @kraken = create(:kraken_exchange)
    @key_binance = create(:api_key, user: @user, exchange: @binance)
    @key_kraken = create(:api_key, user: @user, exchange: @kraken)
    create(:asset, :bitcoin)
    create(:asset, :ethereum)
    @day = ->(n) { Time.utc(2026, 1, n, 12) }
  end

  def tx(type, key: @key_binance, day: 1, at: nil, **attrs)
    defaults = { api_key: key, exchange: key.exchange, entry_type: type, transacted_at: at || @day.call(day) }
    defaults.merge!(quote_currency: nil, quote_amount: nil) if %i[deposit withdrawal swap_in swap_out fee].include?(type)
    create(:account_transaction, **defaults, **attrs)
  end

  def price(symbol, day, usd)
    HistoricalPrice.create!(asset: symbol, currency: 'USD', date: @day.call(day).to_date, price: usd)
  end

  test 'cash spent that the ledger never saw arrive is money in' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 20_000)

    summary = Tracker::Ledger.for(@user)

    assert_equal 20_000.to_d, summary.total_invested_usd,
                 'the venue reported the trade and not the transfer that paid for it'
  end

  test 'a deposit that covers an earlier deficit is not counted twice' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 20_000)
    tx(:deposit, day: 2, base_currency: 'USDC', base_amount: 20_000)
    tx(:buy, day: 3, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 20_000)

    summary = Tracker::Ledger.for(@user)

    assert_equal 40_000.to_d, summary.total_invested_usd,
                 'the second buy spends the deposit; only the first one needed money from nowhere'
  end

  test 'a linked cash transfer is not money from outside' do
    deposit = tx(:deposit, key: @key_kraken, day: 2, base_currency: 'USDC', base_amount: 1_000)
    tx(:withdrawal, day: 1, base_currency: 'USDC', base_amount: 1_000, linked_transaction: deposit)

    summary = Tracker::Ledger.for(@user)

    assert_equal 0.to_d, summary.total_invested_usd,
                 'the cash moved between the user\'s own venues — spending it is what would show where it came from'
  end

  test 'a venue that lends is short because it lent, not because history is missing' do
    alpaca = create(:alpaca_exchange)
    key = create(:api_key, user: @user, exchange: alpaca)
    tx(:deposit, key: key, day: 1, base_currency: 'USD', base_amount: 10_000)
    tx(:buy, key: key, day: 2, base_currency: 'QQQM', base_amount: 10, quote_currency: 'USD', quote_amount: 20_000)

    summary = Tracker::Ledger.for(@user)

    assert_equal 10_000.to_d, summary.total_invested_usd,
                 'settled cash goes negative on borrowed money at a broker; a loan is not a contribution'
  end

  test 'a single-row trade that sells cash keeps the already-net fee rule' do
    tx(:sell, day: 1, base_currency: 'USDT', base_amount: 1_000, quote_currency: 'TRY', quote_amount: 30_000,
              fee_amount: 5, fee_currency: 'USDT')

    summary = Tracker::Ledger.for(@user)

    assert_equal 1_000.to_d, summary.total_invested_usd,
                 'the fee is on top only where the row is a settlement leg with no quote of its own'
  end

  test 'a cash deposit whose fee exceeds it arrives at zero rather than in deficit' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 5, fee_amount: 10, fee_currency: 'USDC')

    summary = Tracker::Ledger.for(@user)

    assert_equal 5.to_d, summary.total_invested_usd, 'nothing was borrowed to pay that fee'
  end

  test 'a fee booked before the sale that pays for it does not invent a deficit' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 20_000)
    tx(:fee, day: 2, base_currency: 'USDC', base_amount: 78, at: @day.call(2))
    tx(:sell, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 30_000,
              at: @day.call(2) + 1.hour)

    summary = Tracker::Ledger.for(@user)

    assert_equal 20_000.to_d, summary.total_invested_usd,
                 'the day ends nearly 10k up — the fee came out of the sale, not out of nowhere'
  end

  test 'a venue that reports its funding books the deposit and nothing more' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 20_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 20_000)

    summary = Tracker::Ledger.for(@user)

    assert_equal 20_000.to_d, summary.total_invested_usd
  end

  test 'a ledger warmed in the app zone is found from any other' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)

    Tracker::Ledger.compute!(@user)

    assert_not_nil Time.use_zone('Tallinn') { Tracker::Ledger.cached(@user) },
                   'the cache key must not move with the zone of whoever asks'
  end

  test 'partial sell: FIFO remaining lots make the open position, realised is proceeds minus FIFO cost and fee' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 30_000)
    tx(:sell, day: 3, base_currency: 'BTC', base_amount: 0.5, quote_currency: 'USD', quote_amount: 20_000,
              fee_amount: 10, fee_currency: 'USD')

    summary = Tracker::Ledger.for(@user)

    assert_equal 1, summary.positions.size
    btc = summary.positions.first
    assert_equal 'BTC', btc.symbol
    assert_equal 1.5.to_d, btc.quantity
    assert_equal 40_000.to_d, btc.cost_usd, 'half of the first lot left at 20k plus the whole second at 30k'
    assert_in_delta 26_666.67, btc.avg_cost_usd.to_f, 0.01
    assert_equal @day.call(1), btc.opened_at
    assert_not btc.incomplete
    assert_equal 9_990.to_d, summary.realised_pnl_usd
    assert_equal 10.to_d, summary.fees_usd
    assert_empty summary.round_trips
    assert_equal 50_000.to_d, summary.total_invested_usd,
                 'the buys spent dollars this venue never reported receiving; the sale returns some of them'
  end

  test 'a sell that empties the lots closes a round-trip and leaves no position' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:sell, day: 5, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 25_000)

    summary = Tracker::Ledger.for(@user)

    assert_empty summary.positions
    trip = summary.round_trips.sole
    assert_equal 'BTC', trip.symbol
    assert_equal @day.call(1), trip.opened_at
    assert_equal @day.call(5), trip.closed_at
    assert_equal 1.to_d, trip.quantity
    assert_equal 20_000.to_d, trip.invested_usd
    assert_equal 25_000.to_d, trip.proceeds_usd
    assert_equal 5_000.to_d, trip.realised_pnl_usd
    assert_equal 5_000.to_d, summary.realised_pnl_usd
  end

  test 'a position closed through several partial sells is one round-trip that aggregates every sell' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:sell, day: 2, base_currency: 'BTC', base_amount: 0.5, quote_currency: 'USD', quote_amount: 12_000, fee_amount: 2, fee_currency: 'USD')
    tx(:sell, day: 4, base_currency: 'BTC', base_amount: 0.5, quote_currency: 'USD', quote_amount: 13_000, fee_amount: 3, fee_currency: 'USD')

    summary = Tracker::Ledger.for(@user)

    trip = summary.round_trips.sole
    assert_equal @day.call(1), trip.opened_at
    assert_equal @day.call(4), trip.closed_at
    assert_equal 1.to_d, trip.quantity, 'avg buy and exit price divide by this'
    assert_equal 20_000.to_d, trip.invested_usd
    assert_equal 25_000.to_d, trip.proceeds_usd
    assert_equal 5.to_d, trip.fees_usd
    assert_equal 4_995.to_d, trip.realised_pnl_usd
    assert_equal 4_995.to_d, summary.realised_pnl_usd
  end

  test 'total invested is money in from outside: fiat and stablecoin deposits minus withdrawals; trades move nothing' do
    tx(:deposit, day: 1, base_currency: 'USD', base_amount: 10_000)
    tx(:withdrawal, day: 2, base_currency: 'USD', base_amount: 2_000)
    tx(:deposit, day: 3, base_currency: 'USDT', base_amount: 3_000)
    tx(:buy, day: 4, base_currency: 'BTC', base_amount: 0.1, quote_currency: 'USD', quote_amount: 5_000)
    tx(:sell, day: 5, base_currency: 'BTC', base_amount: 0.05, quote_currency: 'USD', quote_amount: 3_000)

    summary = Tracker::Ledger.for(@user)

    assert_equal 11_000.to_d, summary.total_invested_usd
    assert_equal %w[BTC], summary.positions.map(&:symbol), 'cash is not a position'
  end

  test 'a linked transfer between two of the user\'s exchanges contributes nothing and keeps the coins and their cost' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    deposit = tx(:deposit, key: @key_kraken, day: 3, base_currency: 'BTC', base_amount: 1)
    tx(:withdrawal, day: 2, base_currency: 'BTC', base_amount: 1, linked_transaction: deposit)

    summary = Tracker::Ledger.for(@user)

    btc = summary.positions.sole
    assert_equal 1.to_d, btc.quantity
    assert_equal 20_000.to_d, btc.cost_usd, 'the lot travelled with its cost'
    assert_equal 20_000.to_d, summary.total_invested_usd, 'the transfer contributes nothing; the buy behind it was funded from outside'
    assert_equal 0.to_d, summary.realised_pnl_usd
  end

  test 'a linked transfer that arrived lighter lost the network fee at zero gain' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    deposit = tx(:deposit, key: @key_kraken, day: 3, base_currency: 'BTC', base_amount: 0.99)
    tx(:withdrawal, day: 2, base_currency: 'BTC', base_amount: 1, linked_transaction: deposit)

    summary = Tracker::Ledger.for(@user)

    btc = summary.positions.sole
    assert_equal 0.99.to_d, btc.quantity
    assert_equal 19_800.to_d, btc.cost_usd
    assert_equal 0.to_d, summary.realised_pnl_usd
  end

  test 'an unlinked crypto deposit opens an assumed-basis position at that day\'s price and counts as money in' do
    price('ETH', 1, 3_000)
    tx(:deposit, day: 1, base_currency: 'ETH', base_amount: 2)

    summary = Tracker::Ledger.for(@user)

    eth = summary.positions.sole
    assert_equal 'ETH', eth.symbol
    assert_equal 2.to_d, eth.quantity
    assert_equal 6_000.to_d, eth.cost_usd
    assert eth.incomplete, 'a deposit has no fill price — the basis is assumed, and the page should say so'
    assert_equal 6_000.to_d, summary.total_invested_usd
  end

  test 'an unlinked withdrawal removes lots at cost with no P/L, and takes that value out of total invested' do
    price('BTC', 2, 25_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:withdrawal, day: 2, base_currency: 'BTC', base_amount: 0.4)

    summary = Tracker::Ledger.for(@user)

    btc = summary.positions.sole
    assert_equal 0.6.to_d, btc.quantity
    assert_equal 12_000.to_d, btc.cost_usd, 'the coins left at their FIFO cost — the holdings card shows what the exchange still holds'
    assert_equal 0.to_d, summary.realised_pnl_usd
    assert_equal 10_000.to_d, summary.total_invested_usd,
                 '20k came in to buy the coin, 0.4 BTC at the day\'s 25k went somewhere untracked'
  end

  test 'scoped to one exchange, a coin that moved venues shows on the venue it is on, and the venues\' money-in reflects the move' do
    price('BTC', 2, 28_000)
    price('BTC', 3, 30_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    deposit = tx(:deposit, key: @key_kraken, day: 3, base_currency: 'BTC', base_amount: 1)
    tx(:withdrawal, day: 2, base_currency: 'BTC', base_amount: 1, linked_transaction: deposit)

    on_binance = Tracker::Ledger.for(@user, exchange: @binance)
    on_kraken = Tracker::Ledger.for(@user, exchange: @kraken)

    assert_empty on_binance.positions
    assert_equal 0.to_d, on_binance.realised_pnl_usd, 'a transfer out is not a sale'
    assert_equal(-8_000.to_d, on_binance.total_invested_usd,
                 'per venue: 20k of unreported funding in, the outbound leg out at the day\'s price')
    assert_equal 30_000.to_d, on_kraken.total_invested_usd
    kraken_btc = on_kraken.positions.sole
    assert_equal 1.to_d, kraken_btc.quantity
    assert_equal 30_000.to_d, kraken_btc.cost_usd
    assert kraken_btc.incomplete, 'per venue the arriving coin has no basis of its own'
  end

  test 'fiat and stablecoins never appear as positions; fees sum the priced fees; a missing price marks the summary incomplete' do
    tx(:deposit, day: 1, base_currency: 'USDT', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 0.01, quote_currency: 'USDT', quote_amount: 500,
             fee_amount: 0.5, fee_currency: 'USDT')
    tx(:deposit, day: 3, base_currency: 'XYZ', base_amount: 10) # no price anywhere

    summary = Tracker::Ledger.for(@user)

    assert_equal %w[BTC XYZ], summary.positions.map(&:symbol).sort
    assert_equal 0.5.to_d, summary.fees_usd
    assert summary.positions.find { |p| p.symbol == 'XYZ' }.incomplete
    assert summary.incomplete
  end

  test 'a standalone in-kind fee row shrinks the position at zero gain and counts as a fee at that day\'s price' do
    price('BTC', 2, 50_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:fee, day: 2, base_currency: 'BTC', base_amount: 0.001)

    summary = Tracker::Ledger.for(@user)

    assert_equal 0.999.to_d, summary.positions.sole.quantity
    assert_equal 0.to_d, summary.realised_pnl_usd
    assert_equal 50.to_d, summary.fees_usd
  end

  test 'a sale that overdraws the lots is not a completed round-trip either' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:sell, day: 2, base_currency: 'BTC', base_amount: 2, quote_currency: 'USD', quote_amount: 50_000)

    summary = Tracker::Ledger.for(@user)

    assert_empty summary.round_trips
    assert summary.incomplete
    assert_equal 30_000.to_d, summary.realised_pnl_usd, 'FIFO: 50,000 proceeds less the 20,000 it could match'
  end

  test 'a sale with no basis is booked the way FIFO books it, marks the summary incomplete and is not a round-trip' do
    tx(:sell, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 25_000)

    summary = Tracker::Ledger.for(@user)

    assert_empty summary.round_trips, 'nothing was opened, so nothing closed'
    assert_empty summary.positions
    assert_equal 25_000.to_d, summary.realised_pnl_usd, 'zero basis — the tax engine\'s number, flagged'
    assert summary.incomplete
  end

  test 'a round-trip measured against an assumed basis is flagged, whatever FIFO books for it' do
    price('ETH', 1, 3_000)
    tx(:deposit, day: 1, base_currency: 'ETH', base_amount: 2)
    tx(:sell, day: 3, base_currency: 'ETH', base_amount: 2, quote_currency: 'USD', quote_amount: 7_000)

    summary = Tracker::Ledger.for(@user)

    trip = summary.round_trips.sole
    assert trip.incomplete, 'the cost it is measured against was the day\'s market price, not a fill'
    assert_equal 1_000.to_d, trip.realised_pnl_usd, 'the engine still books it — the page is what withholds it'
  end

  test 'a return of capital beyond the basis is realised gain, and the basis floors at zero' do
    tx(:buy, key: @key_binance, day: 1, base_currency: 'XYZ', base_amount: 10, quote_currency: 'USD', quote_amount: 1_000)
    tx(:return_of_capital, day: 2, base_currency: 'XYZ', base_amount: 10, quote_currency: 'USD', quote_amount: 1_200)

    summary = Tracker::Ledger.for(@user)

    xyz = summary.positions.sole
    assert_equal 10.to_d, xyz.quantity
    assert_equal 0.to_d, xyz.cost_usd
    assert_equal 200.to_d, summary.realised_pnl_usd, 'the engine\'s excess_roc, which nothing read until now'
  end

  test 'a standalone fiat fee row (a broker\'s USD fee) counts as a fee at face value' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:fee, day: 2, base_currency: 'USD', base_amount: 20)

    summary = Tracker::Ledger.for(@user)

    assert_equal 20.to_d, summary.fees_usd, 'enrich prices fiat-base rows at 0 — the ledger values the fee itself'
    assert_equal 1.to_d, summary.positions.sole.quantity
  end

  test 'a nil-quote trade (Kraken, swaps) is priced from the day\'s historical price' do
    price('BTC', 1, 40_000)
    tx(:buy, key: @key_kraken, day: 1, base_currency: 'BTC', base_amount: 0.5, quote_currency: nil, quote_amount: nil)

    summary = Tracker::Ledger.for(@user)

    assert_equal 20_000.to_d, summary.positions.sole.cost_usd
  end

  test 'a swap pair at the same timestamp chains the basis whichever leg was stored first' do
    price('BTC', 2, 40_000)
    price('ETH', 2, 2_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    # The adapter stores the in-leg first (lower id), the out-leg second — both at the same instant.
    tx(:swap_in, day: 2, base_currency: 'ETH', base_amount: 10, group_id: 'swap-1')
    tx(:swap_out, day: 2, base_currency: 'BTC', base_amount: 0.5, group_id: 'swap-1')

    summary = Tracker::Ledger.for(@user)

    eth = summary.positions.find { |p| p.symbol == 'ETH' }
    assert_equal 10_000.to_d, eth.cost_usd, 'half the BTC lot\'s cost moved into ETH — a swap is not a sale'
    assert_equal 0.to_d, summary.realised_pnl_usd
    assert_equal 10_000.to_d, summary.positions.find { |p| p.symbol == 'BTC' }.cost_usd
  end

  test 'cached is nil until compute!, and a new transaction invalidates it' do
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)

    assert_nil Tracker::Ledger.cached(@user)
    Tracker::Ledger.compute!(@user)
    assert_equal 1, Tracker::Ledger.cached(@user).positions.size

    tx(:buy, day: 2, base_currency: 'ETH', base_amount: 1, quote_currency: 'USD', quote_amount: 3_000)
    assert_nil Tracker::Ledger.cached(@user), 'the key carries the ledger\'s size and last change — no manual invalidation'
  end

  test 'the job computes, caches and refreshes the page' do
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    Turbo::StreamsChannel.expects(:broadcast_refresh_to).with("user_#{@user.id}", :sync)

    Tracker::LedgerJob.perform_now(@user.id)

    assert_equal 1, Tracker::Ledger.cached(@user).positions.size
  end

  test 'the job can compute an exchange-scoped ledger and caches it under its own key' do
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    tx(:buy, key: @key_kraken, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    Turbo::StreamsChannel.stubs(:broadcast_refresh_to)

    Tracker::LedgerJob.perform_now(@user.id, @kraken.id)

    assert_nil Tracker::Ledger.cached(@user)
    assert_equal 1, Tracker::Ledger.cached(@user, exchange: @kraken).positions.size
  end
end
