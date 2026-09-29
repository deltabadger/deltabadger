require 'test_helper'

# Lots sit on the venue that holds them: a sale takes its own venue's lots, a linked transfer
# carries its lots — cost and date — to the venue it went to, and money in follows them. Every
# venue's figures are then a slice of the whole, and the whole is their sum.
class Tracker::LocatedLedgerTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @kraken = create(:kraken_exchange)
    @key_binance = create(:api_key, user: @user, exchange: @binance)
    @key_kraken = create(:api_key, user: @user, exchange: @kraken)
    create(:asset, :bitcoin)
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

  def transfer(amount:, arrives:, from: @key_binance, to: @key_kraken, day: 2, currency: 'BTC')
    deposit = tx(:deposit, key: to, day: day + 1, base_currency: currency, base_amount: arrives)
    tx(:withdrawal, key: from, day: day, base_currency: currency, base_amount: amount, linked_transaction: deposit)
  end

  def assert_sums(scopes, *venues)
    all = scopes.fetch(nil)
    %i[total_invested_usd realised_pnl_usd fees_usd received_usd].each do |figure|
      assert_equal all.public_send(figure), venues.sum(0.to_d) { |venue| scopes.fetch(venue.id).public_send(figure) },
                   "#{figure}: the venues add up to the whole"
    end
    assert_equal all.positions.sum(0.to_d, &:cost_usd),
                 venues.sum(0.to_d) { |venue| scopes.fetch(venue.id).positions.sum(0.to_d, &:cost_usd) }
  end

  test 'a transferred coin keeps its cost on the venue it went to, and money in follows it' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    transfer(amount: 1, arrives: 0.99)

    scopes = Tracker::Ledger.scopes(@user)

    kraken = scopes.fetch(@kraken.id)
    btc = kraken.positions.sole
    assert_equal 0.99.to_d, btc.quantity
    assert_equal 19_800.to_d, btc.cost_usd, 'the lot travelled with its cost'
    assert_not btc.estimated, 'a carried cost is not an assumption'
    assert_equal 19_800.to_d, kraken.total_invested_usd

    binance = scopes.fetch(@binance.id)
    assert_empty binance.positions
    assert_equal 200.to_d, binance.total_invested_usd, 'the network fee is what stayed behind, as a loss'
    assert_equal 0.to_d, binance.realised_pnl_usd

    all = scopes.fetch(nil)
    assert_equal 20_000.to_d, all.total_invested_usd
    assert_equal 19_800.to_d, all.positions.sole.cost_usd
    assert_sums(scopes, @binance, @kraken)
  end

  test 'a sale takes the lots of the venue it happened on' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 10_000)
    tx(:buy, key: @key_kraken, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 30_000)
    tx(:sell, key: @key_kraken, day: 3, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 40_000)

    scopes = Tracker::Ledger.scopes(@user)

    assert_equal 10_000.to_d, scopes.fetch(@kraken.id).realised_pnl_usd, 'the Kraken coin cost 30k'
    assert_equal 10_000.to_d, scopes.fetch(nil).realised_pnl_usd
    assert_equal 10_000.to_d, scopes.fetch(@binance.id).positions.sole.cost_usd, 'Binance still holds its own coin'
    assert_empty scopes.fetch(@kraken.id).positions
    assert_sums(scopes, @binance, @kraken)
  end

  test 'a venue that sends more than its history holds opens what it must have had, and that travels' do
    price('BTC', 1, 20_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 0.5, quote_currency: 'USD', quote_amount: 10_000)
    transfer(amount: 1, arrives: 1)

    scopes = Tracker::Ledger.scopes(@user)

    btc = scopes.fetch(@kraken.id).positions.sole
    assert_equal 1.to_d, btc.quantity
    assert_equal 20_000.to_d, btc.cost_usd, 'half bought, half opened at the day\'s price'
    assert btc.estimated, 'the opened half is an assumption, wherever it went'
    assert_empty scopes.fetch(@binance.id).positions
    assert_sums(scopes, @binance, @kraken)
  end

  test 'a transfer back into the same venue moves nothing but its fee' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    transfer(amount: 1, arrives: 0.99, to: @key_binance)

    scopes = Tracker::Ledger.scopes(@user)

    btc = scopes.fetch(@binance.id).positions.sole
    assert_equal 0.99.to_d, btc.quantity
    assert_equal 19_800.to_d, btc.cost_usd
    assert_equal 20_000.to_d, scopes.fetch(@binance.id).total_invested_usd
    assert_nil scopes[@kraken.id]
  end

  test 'cash moved between venues moves its money in, and the whole does not change' do
    alpaca = create(:alpaca_exchange)
    key_alpaca = create(:api_key, user: @user, exchange: alpaca)
    tx(:deposit, key: @key_kraken, day: 1, base_currency: 'USD', base_amount: 1_000)
    transfer(amount: 1_000, arrives: 1_000, from: @key_kraken, to: key_alpaca, currency: 'USD')

    scopes = Tracker::Ledger.scopes(@user)

    assert_equal 0.to_d, scopes.fetch(@kraken.id).total_invested_usd
    assert_equal 1_000.to_d, scopes.fetch(alpaca.id).total_invested_usd
    assert_equal 1_000.to_d, scopes.fetch(nil).total_invested_usd
    assert_equal({ 'USD' => 1_000.to_d }, scopes.fetch(alpaca.id).cash.to_h)
    assert_sums(scopes, @kraken, alpaca)
  end

  # Wash-sale arming reads the account-wide FIFO it always has: which lot a sale consumed is a tax
  # question, and per-venue matching would stop arming a loss the tax engine still sees.
  test 'loss sales are judged by the account-wide walk, not the located one' do
    tx(:buy, at: 20.days.ago, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 50_000)
    tx(:buy, key: @key_kraken, at: 10.days.ago, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 10_000)
    tx(:sell, key: @key_kraken, at: 5.days.ago, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 30_000)

    scopes = Tracker::Ledger.scopes(@user)

    assert_equal 20_000.to_d, scopes.fetch(@kraken.id).realised_pnl_usd, 'located: the Kraken coin, a gain'
    assert_equal [5.days.ago.to_date], scopes.fetch(nil).loss_sales.values_at('BTC'),
                 'global FIFO sold the 50k Binance coin at a loss, and that still arms'
    assert_equal({}, scopes.fetch(@kraken.id).loss_sales)
  end

  # A sale into USDT credits a cash pot, not a pool of lots — moving it is money moving, never coins
  # moved out of nothing.
  test 'a stablecoin transfer moves money in and marks nothing incomplete' do
    price('BTC', 1, 20_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:sell, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDT', quote_amount: 25_000)
    transfer(amount: 25_000, arrives: 25_000, day: 3, currency: 'USDT')

    scopes = Tracker::Ledger.scopes(@user)

    assert_not scopes.fetch(nil).incomplete
    assert_not scopes.fetch(@kraken.id).incomplete
    assert_equal 25_000.to_d, scopes.fetch(@kraken.id).total_invested_usd, 'the dollars arrived at face'
    assert_equal(-5_000.to_d, scopes.fetch(@binance.id).total_invested_usd,
                 '20k funded the buy, 25k left: the 5k gain went with it')
    assert_equal 5_000.to_d, scopes.fetch(@binance.id).realised_pnl_usd
    assert_sums(scopes, @binance, @kraken)
  end

  test 'a coin whose cost nobody had carries that doubt to the venue it went to' do
    tx(:deposit, day: 1, base_currency: 'BTC', base_amount: 1) # no price anywhere
    transfer(amount: 1, arrives: 1)

    money_in = Tracker::Ledger.money_in(@user)

    arriving = money_in.find { |term| term.exchange == 'kraken' }
    assert_not arriving.complete, 'an unknown basis is not a known zero'
    assert Tracker::Ledger.scopes(@user).fetch(@kraken.id).incomplete
  end

  test 'a sale stamped the same second as the transfer that fed it sells the coins that arrived' do
    at = @day.call(2)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:sell, key: @key_kraken, at: at, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 30_000)
    deposit = tx(:deposit, key: @key_kraken, at: at, base_currency: 'BTC', base_amount: 1)
    tx(:withdrawal, at: at, base_currency: 'BTC', base_amount: 1, linked_transaction: deposit)

    scopes = Tracker::Ledger.scopes(@user)

    assert_empty scopes.fetch(nil).positions, 'nothing is left, and nothing was invented'
    assert_equal 10_000.to_d, scopes.fetch(@kraken.id).realised_pnl_usd, 'sold at 30k the coin that cost 20k'
    assert_empty scopes.fetch(@kraken.id).openings
  end

  test 'a buy and the transfer of what it bought in the same second stay in that order' do
    at = @day.call(1)
    price('BTC', 1, 20_000)
    tx(:buy, at: at, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    deposit = tx(:deposit, key: @key_kraken, at: at, base_currency: 'BTC', base_amount: 1)
    tx(:withdrawal, at: at, base_currency: 'BTC', base_amount: 1, linked_transaction: deposit)

    all = Tracker::Ledger.scopes(@user).fetch(nil)

    assert_equal 1.to_d, all.positions.sole.quantity, 'nothing opened ahead of the buy'
    assert_equal 20_000.to_d, all.total_invested_usd
  end

  test 'two transfers crossing in the same second each land before what their far venue does with the coin' do
    at = @day.call(2)
    create(:asset, symbol: 'SOL', external_id: 'solana')
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 100)
    tx(:buy, key: @key_kraken, day: 1, base_currency: 'SOL', base_amount: 1, quote_currency: 'USD', quote_amount: 20)
    tx(:sell, key: @key_kraken, at: at, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 150)
    btc_in = tx(:deposit, key: @key_kraken, at: at, base_currency: 'BTC', base_amount: 1)
    sol_in = tx(:deposit, at: at, base_currency: 'SOL', base_amount: 1)
    tx(:withdrawal, key: @key_kraken, at: at, base_currency: 'SOL', base_amount: 1, linked_transaction: sol_in)
    tx(:withdrawal, at: at, base_currency: 'BTC', base_amount: 1, linked_transaction: btc_in)

    all = Tracker::Ledger.scopes(@user).fetch(nil)

    assert_equal %w[SOL], all.positions.map(&:symbol), 'no BTC left, none invented'
    assert_equal 120.to_d, all.total_invested_usd
  end

  test 'a coin swapped on arrival, in the transfer\'s second, carries its cost into the swap' do
    at = @day.call(2)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:swap_out, key: @key_kraken, at: at, base_currency: 'BTC', base_amount: 1, group_id: 'g1')
    tx(:swap_in, key: @key_kraken, at: at, base_currency: 'ETH', base_amount: 10, group_id: 'g1')
    deposit = tx(:deposit, key: @key_kraken, at: at, base_currency: 'BTC', base_amount: 1)
    tx(:withdrawal, at: at, base_currency: 'BTC', base_amount: 1, linked_transaction: deposit)
    price('ETH', 2, 3_000)
    price('BTC', 2, 30_000)

    eth = Tracker::Ledger.scopes(@user).fetch(@kraken.id).positions.sole

    assert_equal 'ETH', eth.symbol
    assert_equal 20_000.to_d, eth.cost_usd, 'the BTC cost carried through the swap, not the day\'s market'
  end

  test 'cash that arrives in the second it is spent pays for what it bought' do
    at = @day.call(2)
    tx(:deposit, day: 1, base_currency: 'USDT', base_amount: 1_000)
    deposit = tx(:deposit, key: @key_kraken, at: at, base_currency: 'USDT', base_amount: 1_000)
    tx(:buy, key: @key_kraken, at: at, base_currency: 'BTC', base_amount: 0.01, quote_currency: 'USDT', quote_amount: 1_000)
    tx(:withdrawal, at: at, base_currency: 'USDT', base_amount: 1_000, linked_transaction: deposit)

    all = Tracker::Ledger.scopes(@user).fetch(nil)

    assert_equal 1_000.to_d, all.total_invested_usd, 'no funding invented for a buy the transfer paid for'
    assert_equal 0.to_d, all.cash.values.sum
  end
end
