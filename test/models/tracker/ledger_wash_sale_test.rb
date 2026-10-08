require 'test_helper'

# The bot's own placement lock is the zero-latency layer; this is the correction layer on top, and
# the only one that ever sees a sale the user made on the exchange's own website.
class Tracker::LedgerWashSaleTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    @exchange = create(:binance_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
    @asset = create(:asset, symbol: 'AAA', name: 'Coin AAA', external_id: 'coin-aaa')
    @usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: @asset, quote_asset: @usd)
  end

  def bought(qty, cost, at:)
    create(:account_transaction, api_key: @api_key, entry_type: :buy, base_currency: 'AAA',
                                 base_amount: qty, quote_currency: 'USD', quote_amount: cost,
                                 transacted_at: at)
  end

  def sold(qty, proceeds, at:)
    create(:account_transaction, api_key: @api_key, entry_type: :sell, base_currency: 'AAA',
                                 base_amount: qty, quote_currency: 'USD', quote_amount: proceeds,
                                 transacted_at: at)
  end

  def arm!
    Tracker::LedgerJob.new.perform(@user.id)
  end

  def lock = @user.wash_sale_locks.find_by(asset_id: @asset.id)

  test 'a loss sale no bot made arms a lock, dated from the sale' do
    bought(1, 100, at: 40.days.ago)
    sold(1, 60, at: 3.days.ago)

    arm!

    assert lock, 'the exchange-side sale armed it'
    assert_equal (3.days.ago.to_date + 31).beginning_of_day, lock.buy_locked_until,
                 'dated from the disposal, not from when we noticed'
    assert_predicate lock, :buy_locked?, 'a window added in seconds instead of days would be dead on arrival'
  end

  test 'a fully liquidated position still arms — the balance is gone, the disposal is not' do
    bought(1, 100, at: 40.days.ago)
    sold(1, 60, at: 2.days.ago)
    AccountBalance.where(user_id: @user.id).delete_all

    arm!

    assert_predicate lock, :buy_locked?,
                     'resolving symbols through balances would miss the case this feature exists for'
  end

  test 'a sale that nets a gain but consumed a losing lot still arms' do
    bought(1, 100, at: 60.days.ago)
    bought(1, 20, at: 50.days.ago)
    sold(2, 140, at: 2.days.ago) # net +20, but the 100 lot fetched 70

    arm!

    assert_predicate lock, :buy_locked?,
                     'the ledger layer must judge lot by lot, exactly as Bot::TaxLots does'
  end

  test 'a sale at a gain on every lot arms nothing' do
    bought(1, 100, at: 40.days.ago)
    sold(1, 140, at: 2.days.ago)

    arm!

    assert_empty @user.wash_sale_locks.live
  end

  test 'arming is idempotent and never shortens' do
    bought(1, 100, at: 40.days.ago)
    sold(1, 60, at: 3.days.ago)
    arm!
    first = lock.buy_locked_until

    arm!

    assert_equal first, lock.reload.buy_locked_until
    assert_equal 1, @user.wash_sale_locks.count
  end

  test 'a bot-armed and a ledger-armed lock for the same asset are one row' do
    bought(1, 100, at: 40.days.ago)
    sold(1, 60, at: 1.day.ago)
    WashSaleLock.create!(user: @user, asset: @asset, buy_locked_until: 5.days.from_now)

    arm!

    assert_equal 1, @user.wash_sale_locks.count
  end

  test 'the lock says where it came from' do
    bought(1, 100, at: 40.days.ago)
    sold(1, 60, at: 2.days.ago)

    arm!

    assert_equal 'ledger', lock.source
    assert_empty BotActivityLog.where(event: 'wash_sale_locked'),
                 'a user-level event has no honest home in a bot-scoped feed'
  end

  test 'the rule being off arms nothing' do
    @user.update!(wash_sale_enabled: false)
    bought(1, 100, at: 40.days.ago)
    sold(1, 60, at: 2.days.ago)

    arm!

    assert_empty @user.wash_sale_locks
  end

  test 'a sale older than the window arms nothing' do
    bought(1, 100, at: 90.days.ago)
    sold(1, 60, at: 40.days.ago)

    arm!

    assert_empty @user.wash_sale_locks.live
  end

  # An exchange-scoped job still in the queue from before one walk stated every venue: it is run as
  # the whole-account walk it now is, and arms from the account, never from one venue.
  test 'a job enqueued with an exchange id arms the whole account' do
    bought(1, 100, at: 40.days.ago)
    sold(1, 60, at: 2.days.ago)

    Tracker::LedgerJob.new.perform(@user.id, @exchange.id)

    assert lock
  end

  # Global FIFO sold the older, dearer lot: a loss the tax engine sees. The located walk sold the
  # cheaper lot on the venue the sale happened on — a gain on the page. Arming follows the tax engine.
  test 'arming reads the account-wide FIFO, not the per-venue lots' do
    kraken_key = create(:api_key, user: @user, exchange: create(:kraken_exchange))
    bought(1, 100, at: 20.days.ago)
    create(:account_transaction, api_key: kraken_key, entry_type: :buy, base_currency: 'AAA', base_amount: 1,
                                 quote_currency: 'USD', quote_amount: 20, transacted_at: 10.days.ago)
    create(:account_transaction, api_key: kraken_key, entry_type: :sell, base_currency: 'AAA', base_amount: 1,
                                 quote_currency: 'USD', quote_amount: 60, transacted_at: 2.days.ago)

    arm!

    assert lock, 'the 100 lot sold for 60 is a loss, whichever venue the page puts it on'
  end

  # Rows kept arriving through every pass of the walk: the job asks for a run that WAITS for its
  # guard rather than one the guard would discard, so the sale that arrived is still armed.
  test 'a ledger still moving after every pass asks for a run that waits for the guard' do
    Tracker::Ledger.stubs(:cache_key).returns('a', 'b').then.returns('c', 'd').then.returns('e', 'f').then.returns('g')
    Tracker::LedgerJob::Retry.expects(:perform_later).with(@user.id).once

    arm!

    assert_equal :block, Tracker::LedgerJob::Retry.concurrency_on_conflict
    assert_equal Tracker::LedgerJob.new(@user.id).send(:concurrency_key),
                 Tracker::LedgerJob::Retry.new(@user.id).send(:concurrency_key), 'one guard for both'
  end

  # The proceeds came in a coin nobody could price that day. Valued at zero, the sale read as a loss
  # and locked; it is neither until the price arrives, which the next run asks for again.
  test 'a sale at a price nobody had arms nothing' do
    bought(1, 100, at: 40.days.ago)
    create(:account_transaction, api_key: @api_key, entry_type: :sell, base_currency: 'AAA',
                                 base_amount: 1, quote_currency: 'NOPRICE', quote_amount: 5,
                                 transacted_at: 3.days.ago)

    arm!

    assert_predicate Tracker::Ledger.scopes(@user).fetch(nil), :incomplete, 'the page still says a price is missing'
    assert_empty @user.wash_sale_locks.live
  end
end
