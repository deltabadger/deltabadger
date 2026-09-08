require 'test_helper'

class UserWashSaleTest < ActiveSupport::TestCase
  setup { @user = create(:user) }

  test 'a fresh account has never decided and enforces nothing' do
    assert_nil @user.wash_sale_enabled, 'nil is a state of its own: never decided'
    assert_not_predicate @user, :wash_sale_decided?
    assert_not_predicate @user, :wash_sale_enabled?
    assert_equal 0, @user.wash_sale_days
  end

  test 'deciding no is decided, and looks nothing like never having decided' do
    @user.update!(wash_sale_enabled: false)

    assert_predicate @user, :wash_sale_decided?
    assert_not_predicate @user, :wash_sale_enabled?
    assert_equal 0, @user.wash_sale_days
  end

  test 'the window a switched-off account would use is the first option, never stored' do
    assert_equal Tax::Jurisdictions.wash_sale_options.first.first, @user.wash_sale_jurisdiction
    assert_nil @user.read_attribute(:wash_sale_jurisdiction)
    assert_equal 0, @user.wash_sale_days, 'off is off, whatever the select would show'
  end

  test 'switched on, the days come from the chosen jurisdiction' do
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'IE')
    assert_equal 28, @user.wash_sale_days
  end

  test 'an unknown code is rejected' do
    @user.wash_sale_jurisdiction = 'XX'
    assert_not @user.valid?
  end

  test 'every bot of the account reads the same window' do
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    one = create(:dca_index, user: @user)
    two = create(:dca_multi_asset, user: @user)

    assert_equal 30, one.wash_sale_days
    assert_equal 30, two.wash_sale_days
  end

  test 'a lock one bot wrote is seen by every other bot of the account' do
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    asset = create(:asset, symbol: 'AAA', name: 'Coin AAA', external_id: 'coin-aaa')
    one = create(:dca_index, user: @user)
    two = create(:dca_index, user: @user, exchange: one.exchange, quote_asset: one.quote_asset)
    WashSaleLock.create!(user: @user, asset: asset, buy_locked_until: 10.days.from_now)

    assert_includes one.locked_asset_ids, asset.id
    assert_includes two.locked_asset_ids, asset.id, 'the taxpayer is locked, not one bot'
  end

  test 'switching the rule off releases buying without forgetting the deadlines' do
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    asset = create(:asset, symbol: 'AAA', name: 'Coin AAA', external_id: 'coin-aaa')
    bot = create(:dca_index, user: @user)
    WashSaleLock.create!(user: @user, asset: asset, buy_locked_until: 10.days.from_now)
    assert_includes bot.locked_asset_ids, asset.id

    @user.update!(wash_sale_enabled: false)

    assert_empty bot.locked_asset_ids, 'off releases buying at once'
    assert_equal 1, @user.wash_sale_locks.live.count, 'and the running window is not forgotten'

    @user.update!(wash_sale_enabled: true)
    assert_includes bot.locked_asset_ids, asset.id, 'switching back on resumes it'
  end
end
