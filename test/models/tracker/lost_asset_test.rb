require 'test_helper'

# An asset that is simply gone.
#
# A venue can take a delisted token back (Binance calls it "Asset Recovery"), and the ledger's word
# for that is `lost`. `Tax::Methods::Fifo` has no case for it — buy, deposit, swap, sell, adjustment,
# return_of_capital, fee, withdrawal, and nothing else — so the row passes through and the lots stay.
# The tracker then holds a position in a coin the account does not have, the ledger disagrees with
# the balance, and every P/L that compares them goes silent.
class Tracker::LostAssetTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    create(:asset, :bitcoin)
    @day = ->(n) { Time.utc(2026, 1, n, 12) }
  end

  def tx(type, day:, **attrs)
    create(:account_transaction, api_key: @key, exchange: @binance, entry_type: type,
                                 transacted_at: @day.call(day), quote_currency: nil, quote_amount: nil, **attrs)
  end

  test 'an asset the venue took back leaves the ledger with it' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:lost, day: 5, base_currency: 'BTC', base_amount: 1)

    summary = Tracker::Ledger.for(@user)

    assert_empty summary.positions, 'the coins are gone, so the position is'
  end

  test 'losing part of a stack leaves the rest' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:lost, day: 5, base_currency: 'BTC', base_amount: 0.25)

    position = Tracker::Ledger.for(@user).positions.sole

    assert_equal 0.75.to_d, position.quantity
    assert_equal 15_000.to_d, position.cost_usd, 'the basis leaves with the coins'
  end

  # Nothing came back for them, so what they cost is simply gone: a realised loss of exactly the
  # basis. Money in stays — it was put in — and the loss shows where a loss belongs, so what is
  # held less what went in still equals what was banked plus what is still riding.
  test 'losing something realises its basis as a loss' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:lost, day: 5, base_currency: 'BTC', base_amount: 1)

    summary = Tracker::Ledger.for(@user)

    assert_equal(-20_000.to_d, summary.realised_pnl_usd)
    assert_equal 20_000.to_d, summary.total_invested_usd, 'nothing was credited back'
  end

  # Its proceeds are known exactly — zero — so no chart is needed to state the loss.
  test 'a lost coin nobody can price is still a complete loss' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:lost, day: 5, base_currency: 'BTC', base_amount: 1)

    summary = Tracker::Ledger.for(@user)

    assert_not summary.incomplete
    assert_equal(-20_000.to_d, summary.realised_pnl_usd)
  end
end
