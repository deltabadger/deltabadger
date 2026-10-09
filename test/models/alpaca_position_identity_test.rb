require 'test_helper'

class AlpacaPositionIdentityTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:alpaca_exchange)
    @exchange.stubs(:dry_run?).returns(false)
    @usd = create(:asset, :usd)
    @coin = create(:asset, :bitcoin)
    @stock = create(:asset, external_id: 'BTC.US', symbol: 'BTC', category: 'Stock')
    @client = mock
    @exchange.stubs(:client).returns(@client)
    @client.stubs(:get_account).returns(Result::Success.new({ 'cash' => '100' }))
    [@usd, @coin, @stock].each { |a| create(:exchange_asset, exchange: @exchange, asset: a) }
  end

  def listings(coin_first, available)
    Ticker.where(exchange: @exchange).delete_all
    order = coin_first ? [@coin, @stock] : [@stock, @coin]
    order.each do |asset|
      create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @usd,
                      ticker: asset == @coin ? 'BTC/USD' : 'BTC', base: 'BTC', quote: 'USD', available: available)
    end
  end

  def positions(rows)
    @client.stubs(:get_positions).returns(Result::Success.new(rows))
  end

  test 'positions and rebalancer retain class in either ticker order and availability state' do
    [true, false].product([true, false]).each do |coin_first, available|
      listings(coin_first, available)
      positions([{ 'symbol' => 'BTC', 'asset_class' => 'us_equity', 'qty' => '10' }])
      balances = @exchange.get_balances.data
      assert_equal 10, balances.fetch(@stock.id)[:free]
      assert_equal 0, balances.fetch(@coin.id)[:free]
      bot = Bots::DcaMultiAsset.new(exchange: @exchange)
      bot.stubs(:get_balance).with(asset_id: @stock.id).returns(@exchange.get_balance(asset_id: @stock.id))
      bot.stubs(:get_balance).with(asset_id: @coin.id).returns(@exchange.get_balance(asset_id: @coin.id))
      assert_equal 10, bot.send(:live_free_balance, @stock.id)
      assert_equal 0, bot.send(:live_free_balance, @coin.id)
      positions([{ 'symbol' => 'BTCUSD', 'asset_class' => 'crypto', 'qty' => '2' }])
      balances = @exchange.get_balances.data
      assert_equal 0, balances.fetch(@stock.id)[:free]
      assert_equal 2, balances.fetch(@coin.id)[:free]
    end
  end

  test 'restoring crypto leaves stock balance identity and quantity unchanged' do
    listings(true, true)
    coin = @exchange.tickers.find_by!(base_asset: @coin)
    coin.update!(base: "__stale_#{coin.id}_BTC", ticker: "__stale_#{coin.id}_BTC/USD", available: false)
    positions([{ 'symbol' => 'BTC', 'asset_class' => 'us_equity', 'qty' => '10' }])
    before = @exchange.get_balances.data
    coin.update!(base: 'BTC', ticker: 'BTC/USD', available: true)
    fresh = Exchanges::Alpaca.find(@exchange.id)
    fresh.stubs(:dry_run?).returns(false)
    fresh.stubs(:client).returns(@client)
    assert_equal before, fresh.get_balances.data
    assert_equal 10, before.fetch(@stock.id)[:free]
    assert_equal 0, before.fetch(@coin.id)[:free]
    coin.update!(base: "__stale_#{coin.id}_BTC", ticker: "__stale_#{coin.id}_BTC/USD", available: false)
    positions([{ 'symbol' => 'BTCUSD', 'asset_class' => 'crypto', 'qty' => '2' }])
    assert_equal 2, fresh.get_balances.data.fetch(@coin.id)[:free]
  end

  test 'balance sync keeps both identities and excludes unknown positions' do
    listings(true, false)
    key = create(:api_key, exchange: @exchange)
    key.stubs(:exchange).returns(@exchange)
    @exchange.stubs(:set_client)
    @exchange.stubs(:get_usd_prices).returns(Result::Success.new({ @stock.external_id => 30 }))
    MarketData.stubs(:get_prices).returns(Result::Success.new({ @coin.external_id => 60_000 }))
    positions([{ 'symbol' => 'BTC', 'asset_class' => 'us_equity', 'qty' => '10' },
               { 'symbol' => 'BTC/USD', 'asset_class' => 'crypto', 'qty' => '2' }])
    2.times { assert_predicate AccountBalance::Sync.new(key).sync!, :success? }
    rows = AccountBalance.where(user: key.user, exchange: @exchange)
    assert_equal 10, rows.find_by!(asset: @stock).free
    assert_equal 2, rows.find_by!(asset: @coin).free
    [nil, 'future_class', 'crypto'].product(%w[10 unreadable]).each do |kind, qty|
      positions([{ 'symbol' => 'BTC', 'asset_class' => kind, 'qty' => qty }])
      result = AccountBalance::Sync.new(key).sync!
      assert_predicate result, :success?
      assert_equal [@usd.id], rows.reload.pluck(:asset_id)
    end
  end
  test 'symbol only order and bot creation requests refuse an ambiguous pair' do
    listings(true, true)
    assert_nil BotApi::Orders::Lookup.find_ticker(@exchange, 'BTC', 'USD')
    reader = Object.new.extend(BotApi::Bots::CreateSupport)
    assert_nil reader.send(:find_pair, @exchange, 'BTC', 'USD')
  end

  test 'cash cannot be assigned to a stock with symbol USD' do
    listings(true, true)
    stock_usd = create(:asset, external_id: 'USD.US', symbol: 'USD', category: 'Stock')
    create(:exchange_asset, exchange: @exchange, asset: stock_usd)
    create(:ticker, exchange: @exchange, base_asset: stock_usd, quote_asset: @usd,
                    ticker: 'USD', base: 'USD', quote: 'USD')
    positions([])
    result = @exchange.get_balances
    assert_predicate result, :success?
    assert_equal 100, result.data.fetch(@usd.id)[:free]
    assert_equal 0, result.data.fetch(stock_usd.id)[:free]
  end
  test 'orders and fills use native symbol and reject a contradictory venue class' do
    listings(true, true)
    [['BTC', 'us_equity', @stock], ['BTC/USD', 'crypto', @coin]].each do |symbol, kind, asset|
      row = { 'symbol' => symbol, 'asset_class' => kind, 'filled_qty' => '2', 'filled_avg_price' => '30', 'status' => 'filled' }
      parsed = @exchange.send(:parse_order_data, row, resolve_identity: true)
      assert_equal asset.id, parsed[:ticker].base_asset_id
      assert_equal 2, parsed[:amount_exec]
      assert_equal 60, parsed[:quote_amount_exec]
      assert_raises(ArgumentError) do
        @exchange.send(:parse_order_data, row.merge('asset_class' => kind == 'crypto' ? 'us_equity' : 'crypto'), resolve_identity: true)
      end
    end
  end
  test 'tracker refuses to merge stock and crypto quantities' do
    user = create(:user)
    [[@stock, 10, 30], [@coin, 2, 60_000]].each do |asset, qty, price|
      AccountBalance.create!(user: user, exchange: @exchange, asset: asset, free: qty, locked: 0,
                             usd_price: price, usd_value: qty * price, synced_at: Time.current)
    end
    balances = AccountBalance.for_user(user).to_a
    error = assert_raises(ArgumentError) do
      Tracker::Figures.for(user, ledger: Tracker::Ledger.for(user), balances: balances)
    end
    assert_match(/ambiguous asset class/i, error.message)
  end

  # The catalogue holding both a BTC stock and BTC crypto is not ambiguity: only what this user holds
  # or moved decides it. A crypto-only user with BTC moved since the sync must keep their figures.
  test 'tracker ignores a same-symbol asset of another class the user never touched' do
    user = create(:user)
    AccountBalance.create!(user: user, exchange: @exchange, asset: @coin, free: 2, locked: 0,
                           usd_price: 60_000, usd_value: 120_000, synced_at: Time.current)
    create(:account_transaction, user: user, exchange: @exchange, entry_type: :buy, base_currency: 'BTC',
                                 base_asset: @coin, base_amount: 1, quote_currency: 'USD', quote_amount: 60_000,
                                 transacted_at: Time.current)
    balances = AccountBalance.for_user(user).to_a
    result = Tracker::Figures.for(user, ledger: Tracker::Ledger.for(user), balances: balances, pending: { 'BTC' => 1.to_d })
    assert_not_nil result
  end

  # Fiat and Currency are both cash: two USD rows under the two categories are one class.
  test 'tracker treats the two cash categories of a symbol as one class' do
    user = create(:user)
    fiat = Asset.find_by(symbol: 'USD',
                         category: 'Fiat') || create(:asset, symbol: 'USD', name: 'US Dollar', category: 'Fiat',
                                                             external_id: 'usd-fiat')
    currency = create(:asset, symbol: 'USD', name: 'US Dollar', category: 'Currency', external_id: 'usd-currency')
    [fiat, currency].each do |asset|
      AccountBalance.create!(user: user, exchange: @exchange, asset: asset, free: 10, locked: 0,
                             usd_price: 1, usd_value: 10, synced_at: Time.current)
    end
    balances = AccountBalance.for_user(user).to_a
    assert_nothing_raised { Tracker::Figures.for(user, ledger: Tracker::Ledger.for(user), balances: balances) }
  end

  test 'tracker treats a cash balance and pending cash under the other cash category as one class' do
    user = create(:user)
    fiat = Asset.find_by(symbol: 'USD',
                         category: 'Fiat') || create(:asset, symbol: 'USD', name: 'US Dollar', category: 'Fiat',
                                                             external_id: 'usd-fiat')
    currency = create(:asset, symbol: 'USD', name: 'US Dollar', category: 'Currency', external_id: 'usd-currency')
    AccountBalance.create!(user: user, exchange: @exchange, asset: fiat, free: 10, locked: 0,
                           usd_price: 1, usd_value: 10, synced_at: Time.current)
    create(:account_transaction, user: user, exchange: @exchange, entry_type: :deposit, base_currency: 'USD',
                                 base_asset: currency, base_amount: 5, transacted_at: Time.current,
                                 quote_currency: nil, quote_amount: nil)
    balances = AccountBalance.for_user(user).to_a
    assert_nothing_raised do
      Tracker::Figures.for(user, ledger: Tracker::Ledger.for(user), balances: balances, pending: { 'USD' => 5.to_d })
    end
  end

  # A tokenized share (an xStock) and the company's share are one class: both are added together,
  # as before shared symbols were separated. A stock and a coin under one symbol still refuse.
  test 'tracker treats a stock and its tokenized share under one symbol as one class' do
    user = create(:user)
    token = create(:asset, symbol: 'BTC', name: 'Trust (tokenized)', category: 'Tokenized Stock', external_id: 'btc-tokenized')
    [[@stock, 10, 30], [token, 2, 30]].each do |asset, qty, price|
      AccountBalance.create!(user: user, exchange: @exchange, asset: asset, free: qty, locked: 0,
                             usd_price: price, usd_value: qty * price, synced_at: Time.current)
    end
    balances = AccountBalance.for_user(user).to_a
    assert_nothing_raised { Tracker::Figures.for(user, ledger: Tracker::Ledger.for(user), balances: balances) }
  end

  test 'tracker refuses when the user moved one class of a symbol and holds the other' do
    user = create(:user)
    AccountBalance.create!(user: user, exchange: @exchange, asset: @coin, free: 2, locked: 0,
                           usd_price: 60_000, usd_value: 120_000, synced_at: Time.current)
    create(:account_transaction, user: user, exchange: @exchange, entry_type: :buy, base_currency: 'BTC',
                                 base_asset: @stock, base_amount: 10, quote_currency: 'USD', quote_amount: 300,
                                 transacted_at: Time.current)
    balances = AccountBalance.for_user(user).to_a
    error = assert_raises(ArgumentError) do
      Tracker::Figures.for(user, ledger: Tracker::Ledger.for(user), balances: balances, pending: { 'BTC' => 10.to_d })
    end
    assert_match(/ambiguous asset class/i, error.message)
  end
end
