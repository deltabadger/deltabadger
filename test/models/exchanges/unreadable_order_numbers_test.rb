require 'test_helper'

class Exchanges::UnreadableOrderNumbersTest < ActiveSupport::TestCase
  INVALID_NUMBERS = %w[NaN Infinity -Infinity garbage].freeze
  ORDER_FIELDS = {
    kraken: %w[vol_exec cost price vol],
    alpaca: %w[filled_qty filled_avg_price qty notional limit_price]
  }.freeze

  setup do
    Rails.configuration.stubs(:dry_run).returns(false)
  end

  ORDER_FIELDS.each do |venue, fields|
    fields.each do |field|
      INVALID_NUMBERS.each do |value|
        %i[single bulk].each do |poll|
          test "#{venue} #{poll} poll rejects #{value} in #{field} without changing the order" do
            prepare_bot(venue)
            order = waiting_order
            original = order.attributes
            stub_poll(venue, order.external_id, raw_order(venue).merge(field => value))

            assert_raises(ArgumentError) do
              if poll == :single
                Bot::FetchAndUpdateOrderJob.new.perform(order)
              else
                Bot::FetchAndUpdateOpenOrdersJob.new.perform(@bot)
              end
            end

            assert_equal original, order.reload.attributes
            assert_predicate order, :waiting?
          end
        end

        %i[market limit].each do |type|
          test "#{venue} #{type} placement preserves the accepted order with #{value} in #{field}" do
            prepare_bot(venue)
            @bot.set_missed_quote_amount
            @bot.update!(limit_ordered: type == :limit)
            raw = raw_order(venue).merge(field => value)
            if venue == :kraken
              # AddOrder only acknowledges the id; the queued confirmation supplies the fill.
              stub_request(:post, 'https://api.kraken.com/0/private/AddOrder')
                .to_return(**json_response('error' => [], 'result' => { 'txid' => ['placed-order'] }))
              stub_poll(venue, 'placed-order', raw)
              assert_predicate @bot.set_order(order_amount_in_quote: 100.to_d), :success?
              order = @bot.transactions.last
              original = order.attributes

              assert_raises(ArgumentError) { Bot::FetchAndUpdateOrderJob.new.perform(order) }
              assert_equal original, order.reload.attributes
              assert_predicate order, :waiting?
            else
              @client.stubs(:create_order).returns(Result::Success.new(raw))

              pending_before = @bot.pending_quote_amount
              assert_difference -> { @bot.transactions.count }, 1 do
                assert_predicate @bot.set_order(order_amount_in_quote: 100.to_d), :success?
              end
              order = @bot.transactions.last
              assert_equal raw['id'], order.external_id
              assert_predicate order, :waiting?
              assert_nil order.amount_exec
              assert_nil order.quote_amount_exec
              assert_equal pending_before - 100, @bot.pending_quote_amount
              original = order.attributes
              stub_poll(venue, order.external_id, raw)

              assert_raises(ArgumentError) { Bot::FetchAndUpdateOrderJob.new.perform(order) }
              assert_equal original, order.reload.attributes
            end
          end
        end
      end
    end

    test "#{venue} NaN poll with money owed cannot place a second order in the same tick" do
      prepare_bot(venue)
      @bot.update!(started_at: 2.days.ago)
      order = waiting_order
      original = order.attributes
      stub_poll(venue, order.external_id, raw_order(venue).merge(fields.first => 'NaN'))
      assert_operator @bot.pending_quote_amount, :>, 100
      @exchange.expects(:market_buy).never
      @exchange.expects(:limit_buy).never

      assert_no_difference -> { @bot.transactions.count } do
        assert_raises(ArgumentError) { @bot.execute_action }
      end
      assert_equal original, order.reload.attributes
    end

    test "#{venue} finite fills still close orders with exact amounts" do
      prepare_bot(venue)
      order = waiting_order
      stub_poll(venue, order.external_id, raw_order(venue))

      Bot::FetchAndUpdateOrderJob.new.perform(order)

      assert_predicate order.reload, :closed?
      assert_equal BigDecimal('0.002'), order.amount_exec
      assert_equal BigDecimal('100'), order.quote_amount_exec
      assert_equal BigDecimal('50000'), order.price
    end

    INVALID_NUMBERS.each do |value|
      %i[get_last_price get_bid_price get_ask_price].each do |method|
        test "#{venue} #{method} rejects #{value}" do
          prepare_bot(venue)
          if venue == :kraken
            data = { 'a' => [value, '1', '1'], 'b' => [value, '1', '1'], 'c' => [value, '1'],
                     'v' => %w[1 1], 'p' => %w[50000 50000], 't' => [1, 1],
                     'l' => %w[50000 50000], 'h' => %w[50000 50000], 'o' => '50000' }
            stub_request(:get, %r{\Ahttps://api\.kraken\.com/0/public/Ticker(?:\?[^#]*)?\z})
              .to_return(**json_response('error' => [], 'result' => { @bot.ticker.ticker => data }))
          else
            @exchange.stubs(:crypto_ticker?).returns(false)
            @exchange.stubs(:market_data_client).returns(@client)
            @client.stubs(:get_latest_trade).returns(Result::Success.new({ 'trade' => { 'p' => value } }))
            @client.stubs(:get_latest_quote).returns(Result::Success.new({ 'quote' => { 'bp' => value, 'ap' => value } }))
          end

          assert_raises(ArgumentError) { @exchange.public_send(method, ticker: @bot.ticker, force: true) }
        end
      end
    end
  end

  INVALID_NUMBERS.each do |value|
    test "Kraken rejects #{value} in a resting limit price" do
      prepare_bot(:kraken)
      raw = raw_order(:kraken).merge('price' => '0')
      raw['descr'].merge!('ordertype' => 'limit', 'price' => value)
      stub_poll(:kraken, 'order', raw)

      assert_raises(ArgumentError) { @exchange.get_order(order_id: 'order') }
    end

    %i[price amount_exec quote_amount_exec].each do |field|
      test "Kraken rejects #{value} in recovered trade #{field}" do
        prepare_bot(:kraken)
        @client = mock
        @exchange.stubs(:client).returns(@client)
        @client.stubs(:query_orders_info).returns(Result::Success.new({}))
        aggregate = { order_id: 'order', pair: @bot.ticker.ticker, price: 50_000.to_d,
                      amount_exec: '0.002'.to_d, quote_amount_exec: 100.to_d, side: :buy, order_type: :market, raw: [] }
        aggregate[field] = value == 'garbage' ? value : BigDecimal(value)
        @client.stubs(:closed_orders_from_trades).returns(Result::Success.new({ 'order' => aggregate }))

        assert_raises(ArgumentError) { @exchange.get_order(order_id: 'order') }
      end
    end
  end

  test 'Alpaca accepts an unfilled order with absent optional numeric fields' do
    prepare_bot(:alpaca)
    raw = raw_order(:alpaca).merge('status' => 'new', 'filled_qty' => '0', 'filled_avg_price' => nil,
                                   'qty' => nil, 'limit_price' => nil)
    stub_poll(:alpaca, 'order', raw)

    result = @exchange.get_order(order_id: 'order')

    assert_predicate result, :success?
    assert_equal :open, result.data[:status]
    assert_equal 0, result.data[:amount_exec]
    assert_nil result.data[:amount]
  end

  %i[market limit].each do |type|
    test "Alpaca #{type} acceptance with an unreadable fill reserves spend before the next tick" do
      prepare_bot(:alpaca)
      @bot.ticker.update!(ticker: 'AAPL')
      @bot.set_missed_quote_amount
      @bot.update!(limit_ordered: type == :limit)
      raw = { 'id' => 'accepted-1', 'status' => 'filled', 'symbol' => 'AAPL', 'type' => type.to_s,
              'side' => 'buy', 'notional' => '100', 'filled_qty' => '1', 'filled_avg_price' => 'NaN' }
      @client.expects(:create_order).once.returns(Result::Success.new(raw))
      pending_before = @bot.pending_quote_amount

      assert_predicate @bot.set_order(order_amount_in_quote: 100.to_d), :success?
      order = @bot.transactions.find_by!(external_id: 'accepted-1')
      assert_predicate order, :waiting?
      assert_nil order.amount_exec
      assert_nil order.quote_amount_exec
      assert_equal pending_before - 100, @bot.pending_quote_amount
      original = order.attributes
      stub_poll(:alpaca, order.external_id, raw)

      assert_no_difference -> { @bot.transactions.count } do
        assert_raises(ArgumentError) { @bot.execute_action }
      end
      assert_equal original, order.reload.attributes

      order.bot = @bot
      stub_poll(:alpaca, order.external_id, raw.merge('filled_avg_price' => '100'))
      Bot::FetchAndUpdateOrderJob.new.perform(order)
      assert_predicate order.reload, :closed?
      assert_equal 1.to_d, order.amount_exec
      assert_equal 100.to_d, order.quote_amount_exec
    end
  end

  %i[single bulk].each do |poll|
    test "Alpaca #{poll} poll preserves a waiting order when a positive fill has no average price" do
      prepare_bot(:alpaca)
      @bot.ticker.update!(ticker: 'AAPL')
      order = waiting_order
      original = order.attributes
      raw = { 'id' => 'filled-1', 'symbol' => 'AAPL', 'type' => 'market', 'side' => 'buy',
              'status' => 'filled', 'qty' => '1', 'filled_qty' => '1', 'filled_avg_price' => nil,
              'limit_price' => nil, 'notional' => nil }
      stub_poll(:alpaca, order.external_id, raw)

      assert_raises(ArgumentError) do
        if poll == :single
          Bot::FetchAndUpdateOrderJob.new.perform(order)
        else
          Bot::FetchAndUpdateOpenOrdersJob.new.perform(@bot)
        end
      end
      assert_equal original, order.reload.attributes
      assert_predicate order, :waiting?
    end
  end

  INVALID_NUMBERS.each do |value|
    test "Alpaca stock batch omits #{value} while retaining valid and absent prices" do
      prepare_bot(:alpaca)
      @exchange.stubs(:market_data_client).returns(@client)
      @client.expects(:get_snapshots).returns(Result::Success.new(alpaca_snapshots(value)))

      result = @exchange.get_tickers_prices(symbols: %w[AAPL DYNI MSFT], force: true)

      assert_predicate result, :success?
      assert_equal({ 'AAPL' => 200.to_d, 'DYNI' => 0.to_d }, result.data)
    end

    test "Alpaca crypto batch omits #{value} while retaining valid and absent prices" do
      prepare_bot(:alpaca)
      @exchange.stubs(:market_data_client).returns(@client)
      trades = { 'BTC/USD' => { 'p' => '50000' }, 'ETH/USD' => { 'p' => value }, 'SOL/USD' => {} }
      @client.expects(:get_crypto_latest_trade).returns(Result::Success.new('trades' => trades))

      result = @exchange.get_tickers_prices(symbols: %w[BTC/USD ETH/USD SOL/USD], force: true)

      assert_predicate result, :success?
      assert_equal({ 'BTC/USD' => 50_000.to_d, 'SOL/USD' => 0.to_d }, result.data)
    end

    [nil, %w[XXBTZUSD], %w[XXBTZUSD XETHZUSD]].each do |symbols|
      test "Kraken batch contains valid pairs with #{value} and requested symbols #{symbols.inspect}" do
        prepare_bot(:kraken)
        @bot.ticker.update!(ticker: 'XXBTZUSD')
        @client = @exchange.send(:client)
        prices = { 'XXBTZUSD' => { 'c' => %w[50000 1] }, 'XETHZUSD' => { 'c' => [value, '1'] } }
        @client.expects(:get_ticker_information).with.once.returns(Result::Success.new('error' => [], 'result' => prices))
        result = @exchange.get_tickers_prices(symbols: symbols, force: true)

        assert_predicate result, :success?
        assert_equal({ 'XXBTZUSD' => 50_000.to_d }, result.data)
      end
    end

    test "Kraken per-pair fallback omits #{value} without losing the healthy batch price" do
      prepare_bot(:kraken)
      @client = @exchange.send(:client)
      batch = { 'error' => [], 'result' => { 'XXBTZUSD' => { 'c' => %w[50000 1] } } }
      fallback = { 'error' => [], 'result' => { 'XETHZUSD' => { 'c' => [value, '1'] } } }
      @client.expects(:get_ticker_information).with.returns(Result::Success.new(batch))
      @client.expects(:get_ticker_information).with(pair: 'ETHUSD').returns(Result::Success.new(fallback))

      result = @exchange.get_tickers_prices(symbols: %w[XXBTZUSD ETHUSD], force: true)

      assert_predicate result, :success?
      assert_equal({ 'XXBTZUSD' => 50_000.to_d }, result.data)
    end
  end

  test 'Kraken does not inspect price fields of unrequested pairs' do
    prepare_bot(:kraken)
    prices = { 'XXBTZUSD' => { 'c' => %w[50000 1] }, 'XETHZUSD' => {} }
    @exchange.send(:client).expects(:get_ticker_information).with.returns(Result::Success.new('error' => [], 'result' => prices))

    result = @exchange.get_tickers_prices(symbols: ['XXBTZUSD'], force: true)

    assert_predicate result, :success?
    assert_equal({ 'XXBTZUSD' => 50_000.to_d }, result.data)
  end

  test 'Alpaca balance sync updates holdings and cash despite absent and unreadable stock prices' do
    prepare_bot(:alpaca)
    api_key = @bot.user.api_keys.find_by!(exchange: @exchange)
    api_key.exchange = @exchange
    @exchange.stubs(:market_data_client).returns(@client)
    @client.stubs(:get_snapshots).returns(Result::Success.new(alpaca_snapshots('NaN')))
    stocks = %w[AAPL DYNI MSFT].map do |symbol|
      create(:asset, symbol: symbol, external_id: "alpaca_#{symbol}", category: 'Stock')
    end
    cash = @bot.quote_asset
    balances = (stocks + [cash]).to_h { |asset| [asset.id, { free: 2.to_d, locked: 0.to_d }] }
    @exchange.stubs(:get_balances).returns(Result::Success.new(balances))
    MarketData.expects(:get_prices).with(coin_ids: ['alpaca_MSFT'], currency: 'usd').returns(Result::Success.new({}))
    stale = AccountBalance.create!(user: @bot.user, exchange: @exchange, asset: stocks.last,
                                   free: 1, locked: 0, usd_price: 50, usd_value: 50,
                                   synced_at: 1.day.ago, priced_at: 1.day.ago)
    priced_at = stale.priced_at

    result = AccountBalance::Sync.new(api_key).sync!

    assert_predicate result, :success?
    assert_equal 4, result.data.synced
    assert_equal 3, result.data.priced_fresh
    assert_equal 1, result.data.priced_stale
    assert_equal 400.to_d, AccountBalance.find_by!(asset: stocks.first).usd_value
    assert_equal 0.to_d, AccountBalance.find_by!(asset: stocks.second).usd_price
    assert_equal 2.to_d, AccountBalance.find_by!(asset: cash).usd_value
    assert_equal 2.to_d, stale.reload.free
    assert_equal 100.to_d, stale.usd_value
    assert_equal priced_at, stale.priced_at
  end

  private

  def alpaca_snapshots(invalid_price)
    { 'AAPL' => { 'latestTrade' => { 'p' => 200 } },
      'DYNI' => { 'latestQuote' => { 'ap' => 31.02, 'bp' => 20.66 } },
      'MSFT' => { 'latestTrade' => { 'p' => invalid_price } } }
  end

  def prepare_bot(venue)
    @exchange = create(:"#{venue}_exchange")
    @bot = create(:dca_single_asset, :started, exchange: @exchange)
    stub_ticker_ask_price(@bot.ticker, price: 50_000.to_d)
    stub_ticker_last_price(@bot.ticker, price: 50_000.to_d)
    @bot.stubs(:broadcast_below_minimums_warning)
    @bot.stubs(:funds_are_low?).returns(false)
    if venue == :alpaca
      @client = mock
      @exchange.stubs(:client).returns(@client)
    else
      @exchange.set_client(api_key: @bot.user.api_keys.find_by(exchange: @exchange))
    end
  end

  def waiting_order
    create(:transaction, bot: @bot, status: :submitted, external_status: :open,
                         amount_exec: BigDecimal('0.0001'), quote_amount_exec: 5.to_d)
  end

  def raw_order(venue)
    if venue == :kraken
      { 'descr' => { 'pair' => @bot.ticker.ticker, 'ordertype' => 'market', 'type' => 'buy', 'price' => '0' },
        'status' => 'closed', 'vol' => '0.002', 'vol_exec' => '0.002', 'cost' => '100', 'price' => '50000', 'oflags' => '' }
    else
      { 'id' => 'placed-order', 'symbol' => @bot.ticker.ticker, 'asset_class' => 'crypto', 'type' => 'market', 'side' => 'buy', 'status' => 'filled',
        'qty' => '0.002', 'notional' => '100', 'filled_qty' => '0.002', 'filled_avg_price' => '50000', 'limit_price' => '50000' }
    end
  end

  def stub_poll(venue, order_id, raw)
    if venue == :kraken
      stub_request(:post, 'https://api.kraken.com/0/private/QueryOrders')
        .to_return(**json_response('error' => [], 'result' => { order_id => raw }))
    else
      @client.stubs(:get_order).with(order_id: order_id).returns(Result::Success.new(raw))
    end
  end

  def json_response(data)
    { status: 200, body: data.to_json, headers: { 'Content-Type' => 'application/json' } }
  end
end
