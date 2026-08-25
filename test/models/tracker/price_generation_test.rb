require 'test_helper'

# A ledger and a portfolio history are both READINGS OF PRICES, and prices arrive late. The archive
# answers for a date it could not answer for yesterday; a provider backfills a gap. Neither of the
# two things built on top of them notices on its own:
#
#   * the ledger's cache key follows the TRANSACTIONS, so a healed price leaves a poisoned summary
#     cached until the user happens to trade again — which for a closed position is never;
#   * the snapshot table is written once per day forwards, so a day already swept is never revisited.
#
# Both are pinned to the same marker: the high-water mark of the price table. It only moves when a
# price actually arrives, which is exactly when either is worth redoing, and it stops moving once
# the gaps that can be filled are filled — so neither re-runs forever over a date nobody can price.
class Tracker::PriceGenerationTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    create(:asset, :bitcoin)
    create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :buy,
                                 base_currency: 'BTC', base_amount: 1, quote_currency: 'USD',
                                 quote_amount: 20_000, transacted_at: 3.days.ago)
  end

  def store_price(date, price)
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: date, price: price)
  end

  # ── the ledger ───────────────────────────────────────────────────────────────────────────────
  test 'a price arriving retires the summary computed without it' do
    Tracker::Ledger.compute!(@user)
    assert Tracker::Ledger.cached(@user), 'warm to begin with'

    store_price(2.days.ago.to_date, 21_000)

    assert_nil Tracker::Ledger.cached(@user),
               'the summary was read off a price table that has since changed'
  end

  test 'a table that has not changed keeps its summary' do
    store_price(2.days.ago.to_date, 21_000)
    Tracker::Ledger.compute!(@user)

    assert Tracker::Ledger.cached(@user)
  end

  # ── the history ──────────────────────────────────────────────────────────────────────────────
  test 'a history swept before the price arrived is stale' do
    PortfolioSnapshot.create!(user: @user, date: 2.days.ago.to_date, value_usd: 100,
                              invested_usd: 100, partial: true)
    store_price(2.days.ago.to_date, 21_000)

    assert PortfolioSnapshot.stale_prices?(@user)
  end

  test 'a history swept at this generation is not stale' do
    store_price(2.days.ago.to_date, 21_000)
    PortfolioSnapshot.create!(user: @user, date: 2.days.ago.to_date, value_usd: 100,
                              invested_usd: 100, partial: true)
    PortfolioSnapshot.mark_prices_swept!(@user)

    assert_not PortfolioSnapshot.stale_prices?(@user)
  end

  # The point of hanging this on the price table rather than on the flag: a day that is partial for
  # a reason no price can fix (a negative balance, an exchange whose window starts after the
  # funding deposit) must not ask for the sweep again on every single page load.
  test 'a day nothing can price does not ask to be swept again' do
    store_price(2.days.ago.to_date, 21_000)
    PortfolioSnapshot.create!(user: @user, date: 2.days.ago.to_date, value_usd: 100,
                              invested_usd: 100, partial: true)
    PortfolioSnapshot.mark_prices_swept!(@user)

    3.times { assert_not PortfolioSnapshot.stale_prices?(@user) }
  end

  test 'a history with nothing partial in it is never stale' do
    PortfolioSnapshot.create!(user: @user, date: 2.days.ago.to_date, value_usd: 100,
                              invested_usd: 100, partial: false)
    store_price(2.days.ago.to_date, 21_000)

    assert_not PortfolioSnapshot.stale_prices?(@user)
  end

  test 'one user\'s sweep does not answer for another\'s' do
    other = create(:user)
    PortfolioSnapshot.create!(user: @user, date: 2.days.ago.to_date, value_usd: 100,
                              invested_usd: 100, partial: true)
    PortfolioSnapshot.create!(user: other, date: 2.days.ago.to_date, value_usd: 100,
                              invested_usd: 100, partial: true)
    store_price(2.days.ago.to_date, 21_000)
    PortfolioSnapshot.mark_prices_swept!(@user)

    assert_not PortfolioSnapshot.stale_prices?(@user)
    assert PortfolioSnapshot.stale_prices?(other)
  end
end
