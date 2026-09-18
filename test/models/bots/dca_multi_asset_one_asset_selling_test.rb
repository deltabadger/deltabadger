require 'test_helper'

# A one-asset basket sells as the pair bot it replaces did (Bots::DcaSingleAsset::OrderSetter): up to
# the whole free wallet, not only what it bought, sized off the price it places at. No ledger, ranking
# or valuation stands in the way.
class Bots::DcaMultiAssetOneAssetSellingTest < ActiveSupport::TestCase
  def setup
    @base = create(:asset, :bitcoin)
    @bot = create(:dca_multi_asset, user: create(:user), base_assets: [@base])
    # ONE ticker instance, pinned (as dca_multi_asset_selling_test.rb does), with a floor low enough
    # that small test sales place rather than skip (the factory's is 10 quote).
    @ticker = @bot.composition_tickers.sole
    @ticker.update!(minimum_quote_size: 0.5, minimum_base_size: 0.00001)
    @bot.instance_variable_set(:@tickers, [@ticker])
    @bot.stubs(:composition_tickers).returns([@ticker])
    setup_bot_execution_mocks(@bot, price: 100)
    @bot.exchange.stubs(:market_sell).returns(Result::Success.new(order_id: 's-1'))
    @bot.exchange.expects(:market_buy).never
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)
    @bot.set_missed_quote_amount
    @bot.update!(direction: 'selling', sell_quote_amount: 100)
  end

  test 'sells coins it never bought, up to the free balance, without valuing the basket' do
    wallet(free: 0.5)
    @bot.expects(:metrics_with_current_prices).never

    sell_tick

    order = @bot.transactions.sole
    assert_equal ['sell', @base.symbol, 'REGULAR'], [order.side, order.base, order.transaction_type]
    assert_predicate order, :submitted?
    assert_in_delta 0.5, order.amount.to_f, 1e-9, 'the wallet, not the empty ledger, is the ceiling'
  end

  test 'sells the whole amount when the wallet holds more than the bot bought' do
    wallet(free: 10)

    sell_tick

    assert_in_delta 1.0, @bot.transactions.sole.amount.to_f, 1e-9, '100 quote at a bid of 100'
  end

  test 'does not subtract its own resting sell: the venue already holds it back from the free balance' do
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                         external_id: 'resting', side: :sell, transaction_type: 'REGULAR', base: @base.symbol,
                         quote: @bot.quote_asset.symbol, price: 100, amount: 0.3, amount_exec: nil, quote_amount_exec: nil)
    wallet(free: 0.5)

    sell_tick

    assert_in_delta 0.5, @bot.transactions.where.not(external_id: 'resting').sole.amount.to_f, 1e-9
  end

  test 'an empty wallet skips the tick and keeps running' do
    wallet(free: 0)

    assert_predicate sell_tick, :success?
    assert_empty @bot.transactions
  end

  test 'a failed balance read raises instead of reading as an empty wallet' do
    @bot.exchange.stubs(:get_balances).returns(Result::Failure.new('Invalid API key'))

    assert_raises(RuntimeError) { sell_tick }
    assert_empty @bot.transactions
  end

  test 'a stale bulk price read does not stop a sale' do
    wallet(free: 10)
    @bot.stubs(:metrics_with_current_prices).returns(asset_values: {}, prices_stale: true)

    sell_tick

    assert_predicate @bot.transactions.sole, :submitted?
  end

  test 'a limit sell for N quote is sized at the adjusted limit price, like the pair bot' do
    wallet(free: 10)
    @bot.stubs(:limit_ordered?).returns(true)
    @bot.stubs(:limit_order_pcnt_distance_decimal).returns(0.01.to_d)
    @bot.exchange.stubs(:limit_sell).returns(Result::Success.new(order_id: 'l-1'))

    sell_tick

    order = @bot.transactions.sole
    assert_equal 'limit_order', order.order_type
    assert_in_delta 101, order.price.to_f, 1e-9
    assert_in_delta 100.0 / 101, order.amount.to_f, 1e-6
  end

  test 'a sale below the venue floor writes one skipped row' do
    wallet(free: 10)
    @bot.exchange.expects(:market_sell).never

    sell_tick(0.1)

    assert_equal ['skipped'], @bot.transactions.pluck(:status)
  end

  test 'selling coins the bot never bought takes no wash-sale lock' do
    @bot.user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    wallet(free: 10)

    sell_tick

    assert_predicate @bot.transactions.sole, :submitted?
    assert_empty @bot.user.locked_asset_ids, 'no lot of its own was sold, so there is no loss of its own to protect'
  end

  private

  def sell_tick(amount = 100) = @bot.set_orders(total_orders_amount_in_quote: amount.to_d, side: :sell)

  def wallet(free:)
    stub_exchange_balances(@bot.exchange, @bot.quote_asset_id => { free: 0, locked: 0 },
                                          @base.id => { free:, locked: 0 })
  end
end
