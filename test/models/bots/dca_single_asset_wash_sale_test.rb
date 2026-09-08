require 'test_helper'

# The lock is the taxpayer's, so a single-asset bot obeys it too — it just never ARMS one. Its
# metrics walk emits no per-transaction verdict, and Bot::WashSaleGuard reads a missing verdict as a
# loss on purpose, so wiring the fill hook here would lock the asset after a profitable sale. The
# ledger arms these instead (Tracker::LedgerJob).
class Bots::DcaSingleAssetWashSaleTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_single_asset, user: create(:user))
    @user = @bot.user
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
  end

  def lock_it
    WashSaleLock.create!(user: @user, asset_id: @bot.ticker.base_asset_id, buy_locked_until: 10.days.from_now)
  end

  test 'a locked asset skips the tick and keeps the money for the next one' do
    lock_it
    @bot.expects(:create_order).never

    result = @bot.set_order(order_amount_in_quote: 100.to_d)

    assert_predicate result, :success?, 'a tax skip is not a bot failure — ActionJob raises on those'
    assert_equal 0, @bot.transactions.count, 'no row at all: a skipped row reads as below-minimum'
    assert_empty @bot.bot_activity_logs.where("event LIKE '%wash_sale%'"),
                 'logger only — a line per tick for 30 days is not a feed entry'
  end

  test 'the skip happens before the price is fetched' do
    lock_it
    @bot.expects(:side_price).never

    @bot.set_order(order_amount_in_quote: 100.to_d)
  end

  test 'a lock on another asset leaves this bot alone' do
    other = create(:asset, symbol: 'ZZZ', name: 'Coin ZZZ', external_id: 'coin-zzz')
    WashSaleLock.create!(user: @user, asset: other, buy_locked_until: 10.days.from_now)

    assert_not_includes @user.locked_asset_ids, @bot.ticker.base_asset_id
  end

  test 'the rule being off releases the buy at once' do
    lock_it
    @user.update!(wash_sale_enabled: false)

    assert_empty @user.locked_asset_ids
  end

  test 'a SELLING bot is never blocked by a lock on its own asset' do
    lock_it
    @bot.set_missed_quote_amount
    @bot.update!(direction: 'selling')
    @bot.stubs(:sellable_base_amount).returns(1.to_d)
    # Stubbed at the order-data boundary: everything below it reads the venue, and the only thing
    # under test is that the lock does not stand in the way of a SALE.
    @bot.stubs(:get_order_data).returns(Result::Success.new(
                                          ticker: @bot.ticker, price: 100.to_d, amount: 1.to_d,
                                          quote_amount: 100.to_d, side: :sell, order_type: :market_order
                                        ))
    @bot.stubs(:calculate_best_amount_info).returns(below_minimum_amount: false, amount: 1.to_d)
    @bot.stubs(:persist_accepted_order!).returns(@bot.transactions.build)
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)
    @bot.expects(:create_order).once.returns(Result::Success.new(order_id: 'x'))

    @bot.set_order(side: :sell)
  end

  test 'a sell fill on a single-asset bot arms nothing directly' do
    # A missing verdict is read as a loss, so responding to the fill hook here would lock the asset
    # after a PROFITABLE sale. The concern is deliberately not included on this type.
    assert_not @bot.respond_to?(:reconcile_wash_sale_from_fill!),
               'including Bot::WashSaleGuard here would lock every sell, profitable ones included'

    order = @bot.transactions.create!(exchange: @bot.exchange, base: @bot.ticker.base,
                                      quote: @bot.quote_asset.symbol, side: :sell,
                                      transaction_type: 'REGULAR', status: :submitted,
                                      external_status: :open, external_id: 'o1', amount: 1,
                                      price: 90, order_type: :market_order)

    order.update_with_order_data(status: :closed, amount_exec: 1, quote_amount_exec: 90, price: 90,
                                 amount: 1, side: :sell, order_type: :market_order)

    assert_empty @user.wash_sale_locks, 'the ledger is what arms this one'
  end
end
