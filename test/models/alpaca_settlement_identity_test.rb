require 'test_helper'

class AlpacaSettlementIdentityTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:alpaca_exchange)
    @coin = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @ticker = create(:ticker, exchange: @exchange, base_asset: @coin, quote_asset: @usd,
                              ticker: 'BTC/USD', base: 'BTC', quote: 'USD')
    @bot = create(:dca_single_asset, :started, exchange: @exchange, base_asset: @coin, quote_asset: @usd)
    @bot.define_singleton_method(:with_api_key) { |&block| block.call }
    @bot.stubs(:exchange).returns(@exchange)
    @exchange.stubs(:dry_run?).returns(false)
    @client = mock
    @exchange.stubs(:client).returns(@client)
  end

  def row(id, status = 'filled')
    { 'id' => id, 'symbol' => 'BTC/USD', 'asset_class' => 'crypto', 'type' => 'market', 'side' => 'buy',
      'status' => status, 'qty' => '2', 'filled_qty' => status == 'filled' ? '2' : '0', 'filled_avg_price' => '30' }
  end

  def tracked(id)
    create(:transaction, bot: @bot, exchange: @exchange, external_id: id, status: :submitted, external_status: :open,
                         base_asset: @coin, quote_asset: @usd, base: 'BTC', quote: 'USD', amount_exec: 0, quote_amount_exec: 0)
  end

  test 'tombstoned fills and cancellations settle from stored identity without catalogue access' do
    @ticker.update!(ticker: "__stale_#{@ticker.id}_BTC/USD", base: "__stale_#{@ticker.id}_BTC", available: false)
    %w[filled canceled].each do |status|
      order = tracked(status)
      @client.stubs(:get_order).with(order_id: status).returns(Result::Success.new(row(status, status)))
      @exchange.expects(:tickers).never
      result = @bot.get_order(order_id: status)
      assert_predicate result, :success?
      @exchange.unstub(:tickers)
      assert order.update_with_order_data(result.data)
      assert_equal [@coin.id, @usd.id], order.reload.attributes.values_at('base_asset_id', 'quote_asset_id')
      assert_equal(status == 'filled' ? 'closed' : 'cancelled', order.external_status)
      assert_equal(status == 'filled' ? 60 : 0, order.quote_amount_exec)
    end
  end

  test 'unresolved batch member is logged and skipped while tracked and new tombstone orders update' do
    order = tracked('stored')
    @ticker.update!(ticker: "__stale_#{@ticker.id}_BTC/USD", base: "__stale_#{@ticker.id}_BTC", available: false)
    @client.stubs(:get_order).with(order_id: 'bad').returns(Result::Success.new(row('bad').merge('symbol' => 'UNKNOWN')))
    @client.stubs(:get_order).with(order_id: 'stored').returns(Result::Success.new(row('stored').merge('asset_class' => 'future')))
    @client.stubs(:get_order).with(order_id: 'new').returns(Result::Success.new(row('new')))
    Rails.logger.expects(:warn).with(regexp_matches(/Alpaca order skipped.*identity/))
    result = @bot.get_orders(order_ids: %w[bad stored new])
    assert_predicate result, :success?
    assert_equal %w[new stored], result.data[:orders].keys.sort
    assert_empty result.data[:missing]
    assert order.update_with_order_data(result.data[:orders].fetch('stored'))
    assert_equal 60, order.reload.quote_amount_exec
    assert_equal @coin.id, result.data[:orders].fetch('new').fetch(:ticker).base_asset_id
  end

  test 'unsupported and unmapped positions leave cash funding and the daily notification working' do
    @client.stubs(:get_account).returns(Result::Success.new({ 'cash' => '1' }))
    @client.stubs(:get_positions).returns(Result::Success.new([
                                                                { 'symbol' => 'FUTURE', 'asset_class' => 'future', 'qty' => '10' },
                                                                { 'symbol' => 'UNKNOWN', 'asset_class' => 'us_equity', 'qty' => '10' }
                                                              ]))
    Rails.logger.expects(:warn).with(regexp_matches(/Alpaca position skipped/)).at_least_once
    assert_predicate @bot, :funds_are_low?
    @bot.stubs(:set_order).returns(Result::Success.new)
    @bot.stubs(:broadcast_below_minimums_warning)
    @bot.expects(:notify_end_of_funds).once
    2.times { assert_predicate @bot.execute_action, :success? }
    assert_not_nil @bot.reload.last_end_of_funds_notification
  end

  test 'position catalogue is loaded once for a response with multiple positions' do
    @client.stubs(:get_account).returns(Result::Success.new({ 'cash' => '100' }))
    @client.stubs(:get_positions).returns(Result::Success.new([
                                                                { 'symbol' => 'BTC/USD', 'asset_class' => 'crypto', 'qty' => '2' },
                                                                { 'symbol' => 'BTCUSD', 'asset_class' => 'crypto', 'qty' => '2' }
                                                              ]))
    relation = @exchange.tickers
    @exchange.expects(:tickers).once.returns(relation)
    assert_equal 2, @exchange.get_balances.data.fetch(@coin.id)[:free]
  end
  test 'untracked open orders resolve tombstones and skip unknown or contradictory classes individually' do
    @ticker.update!(ticker: "__stale_#{@ticker.id}_BTC/USD", base: "__stale_#{@ticker.id}_BTC", available: false)
    @client.stubs(:list_orders).returns(Result::Success.new([
                                                              row('bad').merge('asset_class' => 'us_equity'),
                                                              row('missing').except('asset_class'),
                                                              row('good')
                                                            ]))
    Rails.logger.expects(:warn).with(regexp_matches(/Alpaca order skipped.*identity/)).twice
    result = @exchange.list_open_orders
    assert_predicate result, :success?
    assert_equal ['good'], result.data.pluck(:order_id)
    assert_equal @coin.id, result.data.first[:ticker].base_asset_id
  end

  test 'a partially identified order still resolves its missing asset by class' do
    order = tracked('partial')
    order.update_columns(base_asset_id: nil)
    @client.stubs(:get_order).with(order_id: 'partial').returns(Result::Success.new(row('partial')))
    result = @bot.get_order(order_id: 'partial')
    assert_predicate result, :success?
    assert_equal @coin.id, result.data[:ticker].base_asset_id
    assert order.update_with_order_data(result.data)
    assert_equal [@coin.id, @usd.id], order.reload.attributes.values_at('base_asset_id', 'quote_asset_id')
  end

  test 'identity exclusion precedes status and numeric parsing in batch and open orders' do
    %w[status number missing].each do |defect|
      bad = row('bad').merge('symbol' => 'UNKNOWN')
      bad['status'] = 'future_status' if defect == 'status'
      bad['filled_qty'] = 'unreadable' if defect == 'number'
      bad.delete('filled_qty') if defect == 'missing'
      fill = tracked("fill-#{defect}")
      cancel = tracked("cancel-#{defect}")
      @client.stubs(:get_order).with(order_id: 'bad').returns(Result::Success.new(bad))
      @client.stubs(:get_order).with(order_id: fill.external_id).returns(Result::Success.new(row(fill.external_id)))
      @client.stubs(:get_order).with(order_id: cancel.external_id).returns(Result::Success.new(row(cancel.external_id, 'canceled')))
      Rails.logger.expects(:warn).with(regexp_matches(/Alpaca order skipped.*identity/)).twice
      result = @bot.get_orders(order_ids: ['bad', fill.external_id, cancel.external_id])
      assert_predicate result, :success?
      assert_empty result.data[:missing]
      assert_equal [fill.external_id, cancel.external_id].sort, result.data[:orders].keys.sort
      assert fill.update_with_order_data(result.data[:orders].fetch(fill.external_id))
      assert cancel.update_with_order_data(result.data[:orders].fetch(cancel.external_id))
      assert_equal ['closed', 60], [fill.reload.external_status, fill.quote_amount_exec]
      assert_equal ['cancelled', 0], [cancel.reload.external_status, cancel.quote_amount_exec]
      @client.stubs(:list_orders).returns(Result::Success.new([bad, row('good')]))
      assert_equal ['good'], @exchange.list_open_orders.data.pluck(:order_id)
    end
  end
end
