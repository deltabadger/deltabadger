require 'test_helper'

class AccountBalanceTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
  end

  test 'for_user scope filters by user' do
    other_user = create(:user)
    own = create_balance(user: @user, asset: @btc, free: 1)
    create_balance(user: other_user, asset: @btc, free: 1)

    assert_equal [own.id], AccountBalance.for_user(@user).pluck(:id)
  end

  test 'for_exchange scope filters by exchange' do
    kraken = create(:kraken_exchange)
    b1 = create_balance(user: @user, exchange: @exchange, asset: @btc)
    create_balance(user: @user, exchange: kraken, asset: @btc)

    assert_equal [b1.id], AccountBalance.for_exchange(@exchange).where(user: @user).pluck(:id)
  end

  test 'nonzero scope excludes rows with free+locked zero' do
    zero = create_balance(user: @user, asset: @btc, free: 0, locked: 0)
    nonzero = create_balance(user: @user, asset: @eth, free: 0.5)

    ids = AccountBalance.nonzero.pluck(:id)
    assert_includes ids, nonzero.id
    assert_not_includes ids, zero.id
  end

  test 'unique on user/exchange/asset' do
    create_balance(user: @user, exchange: @exchange, asset: @btc)
    dup = AccountBalance.new(user: @user, exchange: @exchange, asset: @btc, free: 2, locked: 0, synced_at: Time.current)
    assert_not dup.valid?
  end

  private

  def create_balance(user:, asset:, exchange: @exchange, free: 1, locked: 0)
    AccountBalance.create!(
      user: user, exchange: exchange, asset: asset,
      free: free, locked: locked, synced_at: Time.current
    )
  end
end
