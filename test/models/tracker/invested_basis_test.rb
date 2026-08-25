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
    defaults.merge!(quote_currency: nil, quote_amount: nil) if %i[deposit withdrawal].include?(type)
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
end
