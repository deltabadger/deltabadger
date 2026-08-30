require 'test_helper'

# What "money in from outside" counts.
#
# A coin ARRIVING is counted at its basis — market on the day it landed, which is the basis the
# engine opens its lot at. A coin LEAVING was counted at market on the day it left, which is the
# same thing a SALE is valued at. It is not a sale — no disposal is recorded, nothing is realised,
# and none of it reaches a tax report — but it debits "money you put in" with the appreciation, and
# once that exceeds the deposits the figure goes NEGATIVE. Money in cannot be negative.
#
# The two directions have to be valued on one scale: in at basis, out at basis. Then a withdrawal
# can never remove more than was put in.
class Tracker::InvestedBasisTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    create(:asset, :bitcoin)
    @day = ->(n) { Time.utc(2026, 1, n, 12) }
  end

  def tx(type, day:, **attrs)
    defaults = { api_key: @key, exchange: @binance, entry_type: type, transacted_at: @day.call(day) }
    defaults.merge!(quote_currency: nil, quote_amount: nil) unless %i[buy sell].include?(type)
    create(:account_transaction, **defaults, **attrs)
  end

  def price(symbol, day, usd)
    HistoricalPrice.create!(asset: symbol, currency: 'USD', date: @day.call(day).to_date, price: usd)
  end

  test 'coins bought and then sent away take only what they cost' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    price('BTC', 5, 4_000) # quadrupled before it left
    tx(:withdrawal, day: 5, base_currency: 'BTC', base_amount: 1)

    summary = Tracker::Ledger.for(@user)

    assert_equal 0.to_d, summary.total_invested_usd,
                 '1,000 in, and the coins that left cost exactly that'
  end

  # The shape that drove it below zero on a real account.
  test 'money in never goes negative, however much the coins appreciated' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 100)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 100)
    price('BTC', 9, 50_000)
    tx(:withdrawal, day: 9, base_currency: 'BTC', base_amount: 1)

    assert_equal 0.to_d, Tracker::Ledger.for(@user).total_invested_usd
  end

  test 'sending away half the stack takes half the cost' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    price('BTC', 5, 9_000)
    tx(:withdrawal, day: 5, base_currency: 'BTC', base_amount: 0.5)

    assert_equal 500.to_d, Tracker::Ledger.for(@user).total_invested_usd
  end

  # Cash is not a position: a dollar taken out is a dollar of capital returned, at its face value.
  test 'cash leaving is still counted at what it is' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:withdrawal, day: 3, base_currency: 'USDC', base_amount: 400)

    assert_equal 600.to_d, Tracker::Ledger.for(@user).total_invested_usd
  end

  # An asset simply taken away is not capital returned — nothing came back for it. The money stays
  # counted as invested and the loss shows up where a loss belongs, in the value.
  test 'an asset lost returns nothing, so nothing is credited back' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    tx(:lost, day: 5, base_currency: 'BTC', base_amount: 1)

    assert_equal 1_000.to_d, Tracker::Ledger.for(@user).total_invested_usd
  end

  # Whatever the tracker does with a transfer, it is not a sale: no disposal, nothing realised.
  test 'sending coins away realises nothing' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    price('BTC', 5, 9_000)
    tx(:withdrawal, day: 5, base_currency: 'BTC', base_amount: 1)

    summary = Tracker::Ledger.for(@user)

    assert_equal 0.to_d, summary.realised_pnl_usd
    assert_empty summary.round_trips
  end

  # ── what arrives without cash ────────────────────────────────────────────────────────────
  #
  # Money in is denominated in BASIS, and so is everything measured against it. A coin that arrived
  # free — a reward, a rebate, an airdrop, a swap credit with no leg behind it — opened a FIFO lot at
  # its market value on arrival; counting it as money in at that same figure is the only way a coin
  # leaving at basis can take out exactly what it brought in. Whether it was taxable income on the
  # day is a jurisdiction's question, not the ledger's.
  test 'coins that arrived free are money in at what they were worth on arrival' do
    price('ETH', 2, 3_000)
    tx(:staking_reward, day: 2, base_currency: 'ETH', base_amount: 2)

    summary = Tracker::Ledger.for(@user)

    assert_equal 6_000.to_d, summary.total_invested_usd
    assert_equal 6_000.to_d, summary.positions.sole.cost_usd, 'the same figure FIFO opened the lot at'
  end

  test 'a rebate paid in a stablecoin is cash from outside' do
    tx(:other_income, day: 2, base_currency: 'USDT', base_amount: 50)

    assert_equal 50.to_d, Tracker::Ledger.for(@user).total_invested_usd
  end

  test 'a rebate paid in euro is cash from outside at that day\'s rate' do
    FxRate.create!(currency: 'USD', date: @day.call(3).to_date, rate: 1.25)
    tx(:other_income, day: 3, base_currency: 'EUR', base_amount: 40)

    assert_equal 50.to_d, Tracker::Ledger.for(@user).total_invested_usd
  end

  test 'a swap credit with nothing behind it is an arrival' do
    price('BNB', 2, 300)
    tx(:swap_in, day: 2, base_currency: 'BNB', base_amount: 1, group_id: nil)

    assert_equal 300.to_d, Tracker::Ledger.for(@user).total_invested_usd
  end

  # Two credits sharing a group are not each other's counterpart: nothing went OUT.
  test 'two swap credits with nothing behind them are two arrivals' do
    price('BNB', 2, 300)
    tx(:swap_in, day: 2, base_currency: 'BNB', base_amount: 1, group_id: 'split')
    tx(:swap_in, day: 2, base_currency: 'BNB', base_amount: 1, group_id: 'split')

    assert_equal 600.to_d, Tracker::Ledger.for(@user).total_invested_usd
  end

  test 'a swap credit paid for with cash is not an arrival' do
    tx(:deposit, day: 1, base_currency: 'USDT', base_amount: 150)
    tx(:swap_out, day: 2, base_currency: 'USDT', base_amount: 150, group_id: 'convert')
    tx(:swap_in, day: 2, base_currency: 'LTC', base_amount: 1, group_id: 'convert')

    summary = Tracker::Ledger.for(@user)

    assert_equal 150.to_d, summary.total_invested_usd, 'the deposit, once'
    assert_equal 150.to_d, summary.positions.sole.cost_usd
  end

  # The mirror of an unlinked withdrawal: coins that left for something the record never saw.
  test 'a swap out with nothing in front of it leaves at basis' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    price('BTC', 3, 5_000)
    tx(:swap_out, day: 3, base_currency: 'BTC', base_amount: 1, group_id: 'orphan')

    summary = Tracker::Ledger.for(@user)

    assert_equal 0.to_d, summary.total_invested_usd
    assert_equal 0.to_d, summary.realised_pnl_usd
    assert_empty summary.positions
  end

  # The part of money in that was never paid for — shown beside the total so the tile cannot be read
  # as a claim that it was all deposited. What it WAS (a rebate, an airdrop, a coin from a wallet
  # nobody linked) is the record's per-row business and, after that, a jurisdiction's.
  test 'what arrived without a purchase behind it is stated beside money in' do
    price('ETH', 2, 3_000)
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:staking_reward, day: 2, base_currency: 'ETH', base_amount: 1)
    tx(:other_income, day: 3, base_currency: 'USDT', base_amount: 50)

    summary = Tracker::Ledger.for(@user)

    assert_equal 3_050.to_d, summary.received_usd
    assert_equal 4_050.to_d, summary.total_invested_usd
  end
end
