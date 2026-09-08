require 'test_helper'

class WashSaleLockTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @asset = create(:asset, symbol: 'AAA', name: 'Coin AAA', external_id: 'coin-aaa')
  end

  test 'a lock is live until its deadline passes' do
    lock = WashSaleLock.create!(user: @user, asset: @asset, buy_locked_until: Time.zone.parse('2026-10-08 00:00'))

    travel_to(Time.zone.parse('2026-10-07 23:59')) { assert_predicate lock, :buy_locked? }
    travel_to(Time.zone.parse('2026-10-08 00:00')) { assert_not_predicate lock, :buy_locked? }
  end

  test 'live scopes to the deadlines still ahead' do
    past = WashSaleLock.create!(user: @user, asset: @asset, buy_locked_until: 1.day.ago)
    other = create(:asset, symbol: 'BBB', name: 'Coin BBB', external_id: 'coin-bbb')
    future = WashSaleLock.create!(user: @user, asset: other, buy_locked_until: 10.days.from_now)

    assert_equal [future.id], @user.wash_sale_locks.live.pluck(:id)
    assert_not_includes @user.wash_sale_locks.live.pluck(:id), past.id
  end

  test 'one lock per user and asset' do
    WashSaleLock.create!(user: @user, asset: @asset, buy_locked_until: 1.day.from_now)

    assert_raises(ActiveRecord::RecordNotUnique) do
      WashSaleLock.insert!({ user_id: @user.id, asset_id: @asset.id,
                             created_at: Time.current, updated_at: Time.current })
    end
  end

  test 'a lock survives the bot that made it' do
    bot = create(:dca_index, user: @user)
    WashSaleLock.create!(user: @user, asset: @asset, buy_locked_until: 10.days.from_now)

    bot.destroy!

    assert_equal 1, @user.wash_sale_locks.live.count, 'the taxpayer is still inside the window'
  end
  test 'a ledger confirmation of an OLDER sale leaves a live placement claim alone' do
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    bot = create(:dca_index, user: @user)
    claim = bot.lock_buying!(@asset.id) # a provisional lock this placement owns
    far = @user.wash_sale_locks.find_by(asset_id: @asset.id).buy_locked_until

    # The nightly walk reconfirms a disposal from a week ago: nearer deadline, same row.
    WashSaleLock.confirm!(user: @user, asset_id: @asset.id, from: 7.days.ago.to_date, source: 'ledger')

    lock = @user.wash_sale_locks.find_by(asset_id: @asset.id)
    assert_equal far, lock.buy_locked_until, 'the longer window still stands'
    assert_equal claim[:token], lock.claim_token,
                 'stripping it would leave this placement unable to undo its own lock'
  end

  test 'and that placement can still roll its lock back when it provably failed' do
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    bot = create(:dca_index, user: @user)
    claim = bot.lock_buying!(@asset.id)
    WashSaleLock.confirm!(user: @user, asset_id: @asset.id, from: 7.days.ago.to_date, source: 'ledger')
    confirmed = @user.wash_sale_locks.find_by(asset_id: @asset.id).confirmed_locked_until

    bot.restore_buy_lock!(@asset.id, claim)

    assert_equal confirmed, @user.wash_sale_locks.find_by(asset_id: @asset.id).buy_locked_until,
                 'back to what the real sale earned, and no further'
  end
end
