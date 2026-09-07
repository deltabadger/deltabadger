require 'test_helper'

class Bot::WashSaleGuardTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_index, user: create(:user))
    @asset = create(:asset, symbol: 'AAA', name: 'Coin AAA', external_id: 'coin-aaa')
    @ticker = create(:ticker, exchange: @bot.exchange, base_asset: @asset, quote_asset: @bot.quote_asset)
    @bot.instance_variable_set(:@tickers, [@ticker])
    @bia = BotIndexAsset.create!(bot: @bot, asset: @asset, ticker: @ticker, target_allocation: 1.0, in_index: true, entered_at: Time.current)
  end

  def choose(code)
    @bot.set_missed_quote_amount
    @bot.update!(wash_sale_jurisdiction: code)
  end

  test 'no jurisdiction means no window and no lock' do
    assert_equal 0, @bot.wash_sale_days
    @bot.lock_buying!(@bia.asset_id, ticker: @ticker)
    assert_nil @bia.reload.buy_locked_until
    assert_empty @bot.locked_members
  end

  test 'a holding the composition never recorded gets a row for its lock, and keeps the lock on re-entry' do
    choose('US')
    @bia.destroy!

    @bot.lock_buying!(@asset.id, ticker: @ticker)

    row = @bot.bot_index_assets.find_by(asset_id: @asset.id)
    assert row, 'a row was created to carry the lock'
    assert_predicate row, :buy_locked?
    assert_not row.in_index, 'created as a quitter: the composition never asked for it'

    @bot.send(:update_bot_index_assets, [{ asset_id: @asset.id, ticker_id: @ticker.id, weight: 1.0 }])
    assert row.reload.in_index, 'the refresh let it in'
    assert_predicate row, :buy_locked?, 'and the lock survived'
  end

  test 'the sale day is day zero, buying resumes the day after the window, the countdown is days until then' do
    choose('US')
    assert_equal 30, @bot.wash_sale_days

    travel_to Time.zone.parse('2026-09-07 12:00') do
      assert_nil @bot.lock_buying!(@bia.asset_id, ticker: @ticker), 'returns the previous deadline, which was none'
      assert_equal Time.zone.parse('2026-10-08 00:00'), @bia.reload.buy_locked_until
      assert_predicate @bia, :buy_locked?
      assert_equal [{ symbol: 'AAA', days_left: 31, until: @bia.buy_locked_until, in_index: true }], @bot.locked_members
    end
    travel_to Time.zone.parse('2026-10-07 23:59') do
      assert_predicate @bia.reload, :buy_locked?, 'day 30 is still inside the window'
      assert_equal 1, @bot.locked_members.first[:days_left]
    end
    travel_to Time.zone.parse('2026-10-08 00:00') do
      assert_not_predicate @bia.reload, :buy_locked?
      assert_empty @bot.locked_members
    end
  end

  test 'a second lock never shortens the first, and restore puts the previous deadline back' do
    choose('US')
    travel_to(Time.zone.parse('2026-09-07 12:00')) { @bot.lock_buying!(@bia.asset_id, ticker: @ticker) }
    first = @bia.reload.buy_locked_until

    travel_to Time.zone.parse('2026-09-01 12:00') do
      previous = @bot.lock_buying!(@bia.asset_id, ticker: @ticker) # an earlier-dated sale cannot pull the deadline in
      assert_equal first, previous
      assert_equal first, @bia.reload.buy_locked_until
    end
    travel_to Time.zone.parse('2026-09-20 12:00') do
      previous = @bot.lock_buying!(@bia.asset_id, ticker: @ticker)
      assert_equal Time.zone.parse('2026-10-21 00:00'), @bia.reload.buy_locked_until
      @bot.restore_buy_lock!(@bia.asset_id, previous)
      assert_equal first, @bia.reload.buy_locked_until, 'a failed second sale leaves the first sale protected'
    end
  end

  test 'a rollback is one statement floored by the row itself, so a fill landing at any instant survives it' do
    choose('US')
    travel_to(Time.zone.parse('2026-09-07 12:00')) { @bot.lock_buying!(@bia.asset_id, ticker: @ticker) }
    # A fill's confirmation written straight into the row, as the fill path would from another
    # process at any point during the rollback — there is no read in the rollback for it to slip past.
    @bia.update_column(:confirmed_locked_until, Time.zone.parse('2026-10-12 00:00'))

    @bot.restore_buy_lock!(@bia.asset_id, Time.zone.parse('2026-10-08 00:00'))

    assert_equal Time.zone.parse('2026-10-12 00:00'), @bia.reload.buy_locked_until
    @bot.restore_buy_lock!(@bia.asset_id, nil)
    assert_equal Time.zone.parse('2026-10-12 00:00'), @bia.reload.buy_locked_until, 'a nil previous still floors at the confirmed deadline'
  end

  test 'a rollback cannot erase what a fill confirmed in between' do
    choose('US')
    travel_to(Time.zone.parse('2026-09-07 12:00')) { @bot.lock_buying!(@bia.asset_id, ticker: @ticker) } # sale 1, provisional: Oct 8
    travel_to Time.zone.parse('2026-09-08 12:00') do
      previous = @bot.lock_buying!(@bia.asset_id, ticker: @ticker)                                       # sale 2, provisional: Oct 9
      assert_equal Time.zone.parse('2026-10-08 00:00'), previous
      assert_not @bot.extend_buy_lock!(base: 'AAA', from: Date.current), 'sale 1 fills now and needs Oct 9: already the deadline'
      @bot.restore_buy_lock!(@bia.asset_id, previous)                                                    # sale 2 never left
      assert_equal Time.zone.parse('2026-10-09 00:00'), @bia.reload.buy_locked_until, "sale 1's confirmed Oct 9 stands"
      assert_equal Time.zone.parse('2026-10-09 00:00'), @bia.confirmed_locked_until
    end
  end

  test 'extend from a fill creates a missing lock and lengthens a short one, never shortens' do
    choose('US')
    travel_to Time.zone.parse('2026-09-07 12:00') do
      assert @bot.extend_buy_lock!(base: 'AAA', from: Date.current)
      assert_equal Time.zone.parse('2026-10-08 00:00'), @bia.reload.buy_locked_until
      assert @bot.extend_buy_lock!(base: 'AAA', from: Date.new(2026, 9, 8)), 'filled the next day'
      assert_equal Time.zone.parse('2026-10-09 00:00'), @bia.reload.buy_locked_until
      assert_not @bot.extend_buy_lock!(base: 'AAA', from: Date.new(2026, 9, 1))
      assert_equal Time.zone.parse('2026-10-09 00:00'), @bia.reload.buy_locked_until
    end
  end

  test 'an exited constituent under a lock is reported too' do
    choose('US')
    @bia.update!(in_index: false, exited_at: Time.current)
    @bot.lock_buying!(@bia.asset_id, ticker: @ticker)
    assert_equal false, @bot.locked_members.first[:in_index]
  end

  test 'an unknown code is rejected and a blank one clears the setting' do
    @bot.wash_sale_jurisdiction = 'XX'
    assert_not @bot.valid?
    parsed = @bot.parse_params(ActionController::Parameters.new(wash_sale_jurisdiction: '').permit!)
    assert parsed.key?(:wash_sale_jurisdiction), 'a deliberate None must reach update, not be compacted away'
    assert_nil parsed[:wash_sale_jurisdiction]
  end

  test 'the multi-asset bot carries the same setting' do
    bot = create(:dca_multi_asset, user: create(:user))
    bot.set_missed_quote_amount
    bot.update!(wash_sale_jurisdiction: 'IE')
    assert_equal 28, bot.wash_sale_days
  end
end
