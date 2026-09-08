require 'test_helper'

class Bot::WashSaleGuardTest < ActiveSupport::TestCase
  def setup
    @user = create(:user)
    @bot = create(:dca_index, user: @user)
    @asset = create(:asset, symbol: 'AAA', name: 'Coin AAA', external_id: 'coin-aaa')
    @ticker = create(:ticker, exchange: @bot.exchange, base_asset: @asset, quote_asset: @bot.quote_asset)
    @bot.instance_variable_set(:@tickers, [@ticker])
    @bia = BotIndexAsset.create!(bot: @bot, asset: @asset, ticker: @ticker, target_allocation: 1.0,
                                 in_index: true, entered_at: Time.current)
  end

  def lock = @user.wash_sale_locks.find_by(asset_id: @asset.id)

  def choose(code)
    @bot.user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: code)
  end

  test 'switched off means no window and no lock, whatever jurisdiction is selected' do
    @bot.user.update!(wash_sale_jurisdiction: 'US')

    assert_equal 0, @bot.wash_sale_days, 'off is off: the selected window is what it would be, not what it is'
    @bot.lock_buying!(@asset.id)
    assert_nil lock, 'the rule being off means no row is even created'
    assert_empty @bot.locked_members
  end

  test 'a lock needs no composition row: it belongs to the taxpayer' do
    choose('US')
    @bia.destroy!

    @bot.lock_buying!(@asset.id)

    assert_predicate lock, :buy_locked?, 'the taxpayer is locked, not the composition row'
    assert_equal [@asset.id], @user.wash_sale_locks.live.pluck(:asset_id)
  end

  test 'the bot lists only the locked names it knows about' do
    choose('US')
    stranger = create(:asset, symbol: 'ZZZ', name: 'Coin ZZZ', external_id: 'coin-zzz')
    WashSaleLock.create!(user: @user, asset: stranger, buy_locked_until: 10.days.from_now)
    @bot.lock_buying!(@asset.id)

    assert_equal %w[AAA], @bot.locked_members.map { |m| m[:symbol] },
                 "a lock on an asset this bot never held is not this bot's business"
    assert_equal %w[AAA ZZZ], @user.locked_assets.map { |m| m[:symbol] }.sort,
                 'but the account-wide list has both'
  end

  test 'the sale day is day zero, buying resumes the day after the window, the countdown is days until then' do
    choose('US')
    assert_equal 30, @bot.wash_sale_days

    travel_to Time.zone.parse('2026-09-07 12:00') do
      assert_nil @bot.lock_buying!(@asset.id)[:previous], 'the deadline it replaced, which was none'
      assert_equal Time.zone.parse('2026-10-08 00:00'), lock.reload.buy_locked_until
      assert_predicate lock, :buy_locked?
      assert_equal [{ symbol: 'AAA', days_left: 31, until: lock.buy_locked_until, source: 'bot' }], @bot.locked_members
    end
    travel_to Time.zone.parse('2026-10-07 23:59') do
      assert_predicate lock.reload, :buy_locked?, 'day 30 is still inside the window'
      assert_equal 1, @bot.locked_members.first[:days_left]
    end
    travel_to Time.zone.parse('2026-10-08 00:00') do
      assert_not_predicate lock.reload, :buy_locked?
      assert_empty @bot.locked_members
    end
  end

  test 'a second lock never shortens the first, and restore puts the previous deadline back' do
    choose('US')
    travel_to(Time.zone.parse('2026-09-07 12:00')) { @bot.lock_buying!(@asset.id) }
    first = lock.reload.buy_locked_until

    travel_to Time.zone.parse('2026-09-01 12:00') do
      claim = @bot.lock_buying!(@asset.id) # an earlier-dated sale cannot pull the deadline in
      assert_equal first, claim[:previous]
      assert_equal first, lock.reload.buy_locked_until
    end
    travel_to Time.zone.parse('2026-09-20 12:00') do
      claim = @bot.lock_buying!(@asset.id)
      assert_equal Time.zone.parse('2026-10-21 00:00'), lock.reload.buy_locked_until
      @bot.restore_buy_lock!(@asset.id, claim)
      assert_equal first, lock.reload.buy_locked_until, 'a failed second sale leaves the first sale protected'
    end
  end

  test 'a rollback is one statement floored by the row itself, so a fill landing at any instant survives it' do
    choose('US')
    travel_to(Time.zone.parse('2026-09-07 12:00')) { @bot.lock_buying!(@asset.id) }
    # A fill's confirmation written straight into the row, as the fill path would from another
    # process at any point during the rollback — there is no read in the rollback for it to slip past.
    lock.update_column(:confirmed_locked_until, Time.zone.parse('2026-10-12 00:00'))

    @bot.restore_buy_lock!(@asset.id, { previous: Time.zone.parse('2026-10-08 00:00'),
                                        token: lock.reload.claim_token })

    assert_equal Time.zone.parse('2026-10-12 00:00'), lock.reload.buy_locked_until
    @bot.restore_buy_lock!(@asset.id, { previous: nil, token: lock.claim_token })
    assert_equal Time.zone.parse('2026-10-12 00:00'), lock.reload.buy_locked_until, 'a nil previous still floors at the confirmed deadline'
  end

  test 'a rollback cannot erase what a fill confirmed in between' do
    choose('US')
    travel_to(Time.zone.parse('2026-09-07 12:00')) { @bot.lock_buying!(@asset.id) } # sale 1, provisional: Oct 8
    travel_to Time.zone.parse('2026-09-08 12:00') do
      claim = @bot.lock_buying!(@asset.id)                                          # sale 2, provisional: Oct 9
      assert_equal Time.zone.parse('2026-10-08 00:00'), claim[:previous]
      assert_not @bot.extend_buy_lock!(base: 'AAA', from: Date.current), 'sale 1 fills now and needs Oct 9: already the deadline'
      @bot.restore_buy_lock!(@asset.id, claim) # sale 2 never left
      assert_equal Time.zone.parse('2026-10-09 00:00'), lock.reload.buy_locked_until, "sale 1's confirmed Oct 9 stands"
      assert_equal Time.zone.parse('2026-10-09 00:00'), lock.confirmed_locked_until
    end
  end

  test 'extend from a fill creates a missing lock and lengthens a short one, never shortens' do
    choose('US')
    travel_to Time.zone.parse('2026-09-07 12:00') do
      assert @bot.extend_buy_lock!(base: 'AAA', from: Date.current)
      assert_equal Time.zone.parse('2026-10-08 00:00'), lock.reload.buy_locked_until
      assert @bot.extend_buy_lock!(base: 'AAA', from: Date.new(2026, 9, 8)), 'filled the next day'
      assert_equal Time.zone.parse('2026-10-09 00:00'), lock.reload.buy_locked_until
      assert_not @bot.extend_buy_lock!(base: 'AAA', from: Date.new(2026, 9, 1))
      assert_equal Time.zone.parse('2026-10-09 00:00'), lock.reload.buy_locked_until
    end
  end

  test 'a rollback whose claim was superseded does nothing' do
    choose('US')
    travel_to Time.zone.parse('2026-09-07 12:00') do
      stale = @bot.lock_buying!(@asset.id)          # placement A claims
      @bot.lock_buying!(@asset.id)                  # placement B claims the same day, same deadline

      @bot.restore_buy_lock!(@asset.id, stale)      # A fails after B claimed

      assert_predicate lock.reload, :buy_locked?, "A's rollback must not touch B's claim"
      assert_equal Time.zone.parse('2026-10-08 00:00'), lock.buy_locked_until
    end
  end

  test 'a fill invalidates every outstanding claim, so no rollback can lower it' do
    choose('US')
    travel_to Time.zone.parse('2026-09-07 12:00') do
      claim = @bot.lock_buying!(@asset.id)
      # false: the provisional claim already reached this deadline, so the fill lengthens nothing.
      # What matters is that it CONFIRMS it — the token goes, and no rollback can lower it again.
      assert_not @bot.extend_buy_lock!(base: 'AAA', from: Date.current)

      @bot.restore_buy_lock!(@asset.id, claim)

      assert_equal Time.zone.parse('2026-10-08 00:00'), lock.reload.buy_locked_until,
                   "a confirmed deadline is not a placement's to withdraw"
      assert_nil lock.claim_token
    end
  end

  test 'the ordinary single-claim rollback still releases' do
    choose('US')
    travel_to Time.zone.parse('2026-09-07 12:00') do
      claim = @bot.lock_buying!(@asset.id)

      @bot.restore_buy_lock!(@asset.id, claim)

      assert_nil lock.reload.buy_locked_until
      assert_nil lock.claim_token
    end
  end

  test 'an exited constituent under a lock is reported too' do
    choose('US')
    @bia.update!(in_index: false, exited_at: Time.current)
    @bot.lock_buying!(@asset.id)

    assert_equal %w[AAA], @bot.locked_members.map { |member| member[:symbol] },
                 'a name the composition dropped still has a window to serve'
  end
end
