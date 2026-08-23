require 'test_helper'

# One row per user per day: what the balances were worth and how much money had come in from
# outside. The nightly balance sync appends today's row; the chart reads the series.
class PortfolioSnapshotTest < ActiveSupport::TestCase
  setup do
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
  end

  def balance(asset, free:, price:, priced_at: Time.current)
    AccountBalance.create!(user: @user, exchange: @binance, asset: asset, free: free, locked: 0,
                           usd_price: price, usd_value: price && (free * price), synced_at: Time.current,
                           priced_at: price && priced_at)
  end

  def deposit(usd)
    create(:account_transaction, api_key: @key, entry_type: :deposit, base_currency: 'USD', base_amount: usd,
                                 quote_currency: nil, quote_amount: nil, transacted_at: 3.days.ago)
  end

  test 'record! writes today\'s row from the priced balances and the ledger — computing the ledger when it is cold' do
    balance(@btc, free: 1.5, price: 40_000)
    balance(@eth, free: 10, price: 2_000)
    deposit(30_000)

    travel_to Time.utc(2026, 8, 23, 3) do
      PortfolioSnapshot.record!(@user)
      row = PortfolioSnapshot.for_user(@user).sole
      assert_equal Date.new(2026, 8, 23), row.date
      assert_equal 80_000.to_d, row.value_usd
      assert_equal 30_000.to_d, row.invested_usd, 'a cold ledger is computed here — this runs inside the sync job, never in a request'
      assert_not row.partial

      AccountBalance.find_by(asset: @eth).update!(usd_value: 25_000)
      PortfolioSnapshot.record!(@user)
      assert_equal 1, PortfolioSnapshot.for_user(@user).count, 'several syncs a day: last one wins'
      assert_equal 85_000.to_d, PortfolioSnapshot.for_user(@user).sole.value_usd
    end
  end

  test 'record! marks the row partial when a held balance has no price, or when prices lag the sync' do
    balance(@btc, free: 1, price: 40_000)
    balance(@eth, free: 5, price: nil)
    PortfolioSnapshot.record!(@user)
    assert PortfolioSnapshot.for_user(@user).sole.partial

    AccountBalance.find_by(asset: @eth).update!(usd_price: 2_000, usd_value: 10_000, priced_at: 20.minutes.ago, synced_at: Time.current)
    PortfolioSnapshot.record!(@user)
    assert PortfolioSnapshot.for_user(@user).sole.partial, 'a stale price is not today\'s value'
  end

  test 'record! marks the row partial when a trading key failed to sync' do
    balance(@btc, free: 1, price: 40_000)
    @key.update!(last_sync_error: 'boom')
    PortfolioSnapshot.record!(@user)
    assert PortfolioSnapshot.for_user(@user).sole.partial
  end

  test 'record! writes a zero row after a liquidation, and nothing for a user with neither balances nor transactions' do
    deposit(1_000)
    PortfolioSnapshot.record!(@user)
    assert_equal 0.to_d, PortfolioSnapshot.for_user(@user).sole.value_usd, 'a sold-out account is a real day on the chart'

    stranger = create(:user)
    PortfolioSnapshot.record!(stranger)
    assert_not PortfolioSnapshot.for_user(stranger).exists?
  end
end
