require 'test_helper'

# The one-off history: a single forward sweep over the ledger, one price-range fetch per symbol,
# last-observed price carried over holes, cash counted at face value, `partial` when a held symbol
# had no price at all. Idempotent: rerunning upserts the same rows.
class PortfolioSnapshot::BackfillJobTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    create(:asset, :bitcoin)
    create(:asset, :ethereum)
    @d0 = Date.new(2026, 1, 1)
    @day = ->(n) { (@d0 + n).to_time(:utc) + 12.hours }
  end

  def tx(type, day:, **attrs)
    defaults = { api_key: @key, entry_type: type, transacted_at: @day.call(day) }
    defaults.merge!(quote_currency: nil, quote_amount: nil) if %i[deposit withdrawal].include?(type)
    create(:account_transaction, **defaults, **attrs)
  end

  def price(symbol, day, usd)
    HistoricalPrice.create!(asset: symbol, currency: 'USD', date: @d0 + day, price: usd)
  end

  def seed_ledger
    tx(:deposit, day: 0, base_currency: 'USD', base_amount: 12_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 10_000)
    tx(:buy, day: 3, base_currency: 'ETH', base_amount: 1, quote_currency: 'USD', quote_amount: 1_000)
    tx(:sell, day: 5, base_currency: 'BTC', base_amount: 0.5, quote_currency: 'USD', quote_amount: 6_000)
  end

  def seed_prices(btc_gap_on_day2: true)
    { 0 => 9_000, 1 => 10_000, 2 => 10_500, 3 => 11_000, 4 => 11_500, 5 => 12_000 }.each do |day, usd|
      next if btc_gap_on_day2 && day == 2

      price('BTC', day, usd)
    end
    { 3 => 1_000, 4 => 1_100, 5 => 1_200 }.each { |day, usd| price('ETH', day, usd) }
  end

  test 'one row per day to yesterday: holdings at the day\'s price plus cash, invested = money in; then every scope recomputes' do
    seed_ledger
    seed_prices
    MarketData.stubs(:get_historical_price_range).returns(Result::Failure.new('offline'))
    Tracker::LedgerJob.expects(:perform_later).with(@user.id).once
    Tracker::LedgerJob.expects(:perform_later).with(@user.id, @binance.id).once

    travel_to @day.call(6) do
      PortfolioSnapshot::BackfillJob.perform_now(@user.id)
    end

    rows = PortfolioSnapshot.for_user(@user).order(:date).to_a
    assert_equal (0..5).map { |n| @d0 + n }, rows.map(&:date)
    assert_equal [12_000, 12_000, 12_000, 13_000, 13_600, 14_200].map(&:to_d), rows.map(&:value_usd),
                 'day 2 carries day 1\'s BTC price; cash is 12k − 10k − 1k + 6k as the days pass'
    assert_equal [12_000.to_d] * 6, rows.map(&:invested_usd), 'deposits move it, trades do not'
    assert rows.none?(&:partial)
  end

  test 'a symbol with no price at all makes its days partial, never a zero-valued holding' do
    seed_ledger
    seed_prices
    tx(:deposit, day: 4, base_currency: 'XYZ', base_amount: 10)
    MarketData.stubs(:get_historical_price_range).returns(Result::Failure.new('offline'))

    travel_to(@day.call(6)) { PortfolioSnapshot::BackfillJob.perform_now(@user.id) }

    rows = PortfolioSnapshot.for_user(@user).order(:date).to_a
    assert_equal [false, false, false, false, true, true], rows.map(&:partial)
    assert_equal 13_600.to_d, rows[4].value_usd, 'the unpriced coin is left out rather than counted at 0'
  end

  test 'prices are fetched once per symbol with a hole, and not at all when the table already covers the range' do
    seed_ledger
    seed_prices(btc_gap_on_day2: true)
    MarketData.expects(:get_historical_price_range).with(has_entries(coin_id: 'bitcoin', currency: 'usd')).once
              .returns(Result::Success.new('prices' => [[@day.call(2).to_i * 1000, 10_500.0]]))
    MarketData.expects(:get_historical_price_range).with(has_entries(coin_id: 'ethereum')).never

    travel_to(@day.call(6)) { PortfolioSnapshot::BackfillJob.perform_now(@user.id) }

    assert_equal 10_500.to_d, HistoricalPrice.lookup(asset: 'BTC', currency: 'USD', date: @d0 + 2),
                 'the fetched point is stored for every later run'
    assert_equal 12_500.to_d, PortfolioSnapshot.for_user(@user).find_by(date: @d0 + 2).value_usd
  end

  test 'a linked transfer contributes nothing, keeps the coins valued minus the network fee; a rerun upserts the same rows' do
    tx(:deposit, day: 0, base_currency: 'USD', base_amount: 10_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 10_000)
    kraken_key = create(:api_key, user: @user, exchange: create(:kraken_exchange))
    deposit = create(:account_transaction, api_key: kraken_key, entry_type: :deposit, base_currency: 'BTC', base_amount: 0.99,
                                           quote_currency: nil, quote_amount: nil, transacted_at: @day.call(3))
    tx(:withdrawal, day: 2, base_currency: 'BTC', base_amount: 1, linked_transaction: deposit)
    (0..4).each { |n| price('BTC', n, 10_000) }
    MarketData.stubs(:get_historical_price_range).returns(Result::Failure.new('offline'))

    travel_to(@day.call(5)) { 2.times { PortfolioSnapshot::BackfillJob.perform_now(@user.id) } }

    rows = PortfolioSnapshot.for_user(@user).order(:date).to_a
    assert_equal 5, rows.size
    assert_equal [10_000, 10_000, 9_900, 9_900, 9_900].map(&:to_d), rows.map(&:value_usd),
                 'the coins never leave the portfolio; the 0.01 network fee does, at the withdrawal'
    assert_equal [10_000.to_d] * 5, rows.map(&:invested_usd)
  end

  test 'fiat cash in another currency is valued through the ECB rate of the day' do
    FxRate.create!(currency: 'USD', date: @d0, rate: 1.25) # 1 EUR = 1.25 USD
    FxRate.create!(currency: 'USD', date: @d0 + 1, rate: 1.20)
    tx(:deposit, day: 0, base_currency: 'EUR', base_amount: 1_000)
    MarketData.stubs(:get_historical_price_range).returns(Result::Failure.new('offline'))

    travel_to(@day.call(2)) { PortfolioSnapshot::BackfillJob.perform_now(@user.id) }

    rows = PortfolioSnapshot.for_user(@user).order(:date).to_a
    assert_equal [1_250.to_d, 1_200.to_d], rows.map(&:value_usd)
    assert_equal [1_250.to_d, 1_250.to_d], rows.map(&:invested_usd), 'money in is valued once, on the day it came in'
  end

  test 'a stock is priced from the stored stock:SYM USD closes the broker fetch leaves behind' do
    alpaca = create(:alpaca_exchange)
    alpaca_key = create(:api_key, user: @user, exchange: alpaca)
    qqqm = create(:asset, symbol: 'QQQM', external_id: 'QQQM.US', category: 'Stock', instrument_type: 'etf')
    create(:ticker, exchange: alpaca, base_asset: qqqm, quote_asset: create(:asset, :usd))
    create(:account_transaction, api_key: alpaca_key, entry_type: :deposit, base_currency: 'USD', base_amount: 1_000,
                                 quote_currency: nil, quote_amount: nil, transacted_at: @day.call(0))
    create(:account_transaction, api_key: alpaca_key, entry_type: :buy, base_currency: 'QQQM', base_amount: 2,
                                 quote_currency: 'USD', quote_amount: 400, transacted_at: @day.call(1))
    Exchanges::Alpaca.any_instance.stubs(:set_client)
    candles = [[@day.call(1).beginning_of_day, 199, 201, 198, 200, 1],
               [@day.call(2).beginning_of_day, 200, 211, 199, 210, 1]]
    Exchanges::Alpaca.any_instance.stubs(:get_candles).returns(Result::Success.new(candles))
    MarketData.stubs(:get_historical_price_range).returns(Result::Failure.new('offline'))

    travel_to(@day.call(3)) { PortfolioSnapshot::BackfillJob.perform_now(@user.id) }

    rows = PortfolioSnapshot.for_user(@user).order(:date).to_a
    assert_equal [1_000.to_d, 1_000.to_d, 1_020.to_d], rows.map(&:value_usd), '600 cash + 2 × 200, then 2 × 210'
    assert_equal 200.to_d, HistoricalPrice.lookup(asset: 'stock:QQQM', currency: 'USD', date: @d0 + 1)
  end

  test 'trade fees leave the balances the way the tax engine books them: quote from cash, base from quantity, third asset from itself' do
    tx(:deposit, day: 0, base_currency: 'USD', base_amount: 20_000)
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 10_000, fee_amount: 20, fee_currency: 'USD')
    tx(:buy, day: 2, base_currency: 'ETH', base_amount: 2, quote_currency: 'USD', quote_amount: 2_000, fee_amount: 0.01, fee_currency: 'ETH')
    tx(:buy, day: 3, base_currency: 'SOL', base_amount: 10, quote_currency: 'USD', quote_amount: 1_000, fee_amount: 0.001, fee_currency: 'BTC')
    # A fee in the disposed asset itself is not consumed again: the adapter already reported the sale net.
    tx(:sell, day: 4, base_currency: 'SOL', base_amount: 5, quote_currency: 'USD', quote_amount: 500, fee_amount: 0.1, fee_currency: 'SOL')
    (0..4).each { |n| { 'BTC' => 10_000, 'ETH' => 1_000, 'SOL' => 100 }.each { |sym, usd| price(sym, n, usd) } }
    create(:asset, symbol: 'SOL', external_id: 'solana')
    MarketData.stubs(:get_historical_price_range).returns(Result::Failure.new('offline'))

    travel_to(@day.call(5)) { PortfolioSnapshot::BackfillJob.perform_now(@user.id) }

    rows = PortfolioSnapshot.for_user(@user).order(:date).to_a
    # day1: cash 20,000 − 10,020 = 9,980 + 1 BTC × 10,000
    # day2: cash 7,980 + BTC 10,000 + 1.99 ETH × 1,000
    # day3: cash 6,980 + 0.999 BTC × 10,000 + 1,990 + 10 SOL × 100
    # day4: cash 7,480 + 9,990 + 1,990 + 5 SOL × 100 (the 0.1 SOL fee is not taken off the 5 that stay)
    assert_equal [20_000.to_d, 19_980.to_d, 19_970.to_d, 19_960.to_d, 19_960.to_d], rows.map(&:value_usd)
  end

  test 'a return of capital adds its cash to the day' do
    tx(:deposit, day: 0, base_currency: 'USD', base_amount: 1_000)
    tx(:buy, day: 1, base_currency: 'XYZ', base_amount: 10, quote_currency: 'USD', quote_amount: 1_000)
    tx(:return_of_capital, day: 2, base_currency: 'XYZ', base_amount: 10, quote_currency: 'USD', quote_amount: 200)
    (0..2).each { |n| price('XYZ', n, 100) }
    MarketData.stubs(:get_historical_price_range).returns(Result::Failure.new('offline'))

    travel_to(@day.call(3)) { PortfolioSnapshot::BackfillJob.perform_now(@user.id) }

    assert_equal [1_000.to_d, 1_000.to_d, 1_200.to_d], PortfolioSnapshot.for_user(@user).order(:date).pluck(:value_usd)
  end

  # A capped ledger window (Binance 90 days, Bybit 7) opens after the funding that paid for the
  # coins, so the sweep meets a sale with nothing behind it.
  test 'a history that starts mid-stream leaves a negative balance, and every day says it is an estimate' do
    tx(:sell, day: 0, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 10_000)
    price('BTC', 0, 10_000)
    price('BTC', 1, 10_000)
    MarketData.stubs(:get_historical_price_range).returns(Result::Failure.new('offline'))

    travel_to(@day.call(2)) { PortfolioSnapshot::BackfillJob.perform_now(@user.id) }

    rows = PortfolioSnapshot.for_user(@user).order(:date).to_a
    assert rows.all?(&:partial), 'the missing acquisition is a hole, not an absence'
    assert_equal [10_000.to_d, 10_000.to_d], rows.map(&:value_usd),
                 'the proceeds are real; the -1 BTC behind them is not counted either way'
  end

  # Inert in the tax engines, which track holdings. Here it is cash the broker kept.
  test 'withholding tax leaves the cash it was taken from' do
    tx(:deposit, day: 0, base_currency: 'USD', base_amount: 1_000)
    create(:account_transaction, api_key: @key, entry_type: :withholding_tax, base_currency: 'USD',
                                 base_amount: 30, quote_currency: nil, quote_amount: nil,
                                 transacted_at: @day.call(1))
    MarketData.stubs(:get_historical_price_range).returns(Result::Failure.new('offline'))

    travel_to(@day.call(2)) { PortfolioSnapshot::BackfillJob.perform_now(@user.id) }

    assert_equal [1_000.to_d, 970.to_d], PortfolioSnapshot.for_user(@user).order(:date).pluck(:value_usd)
  end

  test 'a user without transactions writes nothing' do
    PortfolioSnapshot::BackfillJob.perform_now(@user.id)
    assert_not PortfolioSnapshot.for_user(@user).exists?
  end
end
