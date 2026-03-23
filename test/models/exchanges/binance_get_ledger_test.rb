require 'test_helper'

class Exchanges::BinanceGetLedgerTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
  end

  test 'returns normalized trade entries from account_trade_list' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:account_trade_list).returns(
      Result::Success.new([
                            {
                              'symbol' => 'BTCUSDT',
                              'id' => 123,
                              'orderId' => 456,
                              'price' => '50000.00',
                              'qty' => '0.5',
                              'quoteQty' => '25000.00',
                              'commission' => '25.00',
                              'commissionAsset' => 'USDT',
                              'time' => 1_710_936_000_000,
                              'isBuyer' => true
                            }
                          ])
    )
    honeymaker_client.stubs(:deposit_history).returns(Result::Success.new([]))
    honeymaker_client.stubs(:withdraw_history).returns(Result::Success.new([]))

    @exchange.stubs(:tickers).returns(
      Ticker.where(exchange: @exchange)
    )

    btc = create(:asset, :bitcoin)
    usdt = create(:asset, :usdt)
    create(:ticker, :btc_usdt, exchange: @exchange, base_asset: btc, quote_asset: usdt)

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entries = result.data
    trade = entries.find { |e| e[:entry_type] == :buy }
    assert_not_nil trade
    assert_equal 'BTC', trade[:base_currency]
    assert_equal 0.5, trade[:base_amount]
    assert_equal 'USDT', trade[:quote_currency]
    assert_equal 25_000.0, trade[:quote_amount]
    assert_equal 'USDT', trade[:fee_currency]
    assert_equal 25.0, trade[:fee_amount]
    assert_not_nil trade[:tx_id]
  end

  test 'returns normalized deposit entries' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:account_trade_list).returns(Result::Success.new([]))
    honeymaker_client.stubs(:deposit_history).returns(
      Result::Success.new([
                            {
                              'coin' => 'BTC',
                              'amount' => '1.0',
                              'status' => 1,
                              'txId' => '0xabc123',
                              'insertTime' => 1_710_936_000_000
                            }
                          ])
    )
    honeymaker_client.stubs(:withdraw_history).returns(Result::Success.new([]))

    @exchange.stubs(:tickers).returns(Ticker.none)

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    deposit = result.data.find { |e| e[:entry_type] == :deposit }
    assert_not_nil deposit
    assert_equal 'BTC', deposit[:base_currency]
    assert_equal 1.0, deposit[:base_amount]
    assert_equal '0xabc123', deposit[:tx_id]
  end

  test 'returns normalized withdrawal entries' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:account_trade_list).returns(Result::Success.new([]))
    honeymaker_client.stubs(:deposit_history).returns(Result::Success.new([]))
    honeymaker_client.stubs(:withdraw_history).returns(
      Result::Success.new([
                            {
                              'coin' => 'ETH',
                              'amount' => '2.0',
                              'transactionFee' => '0.005',
                              'status' => 6,
                              'txId' => '0xdef456',
                              'applyTime' => '2026-03-20 10:00:00',
                              'id' => 'w-1'
                            }
                          ])
    )

    @exchange.stubs(:tickers).returns(Ticker.none)

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    withdrawal = result.data.find { |e| e[:entry_type] == :withdrawal }
    assert_not_nil withdrawal
    assert_equal 'ETH', withdrawal[:base_currency]
    assert_equal 2.0, withdrawal[:base_amount]
    assert_equal 'ETH', withdrawal[:fee_currency]
    assert_equal 0.005, withdrawal[:fee_amount]
  end

  test 'detects crypto-to-crypto swaps' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:account_trade_list).returns(
      Result::Success.new([
                            {
                              'symbol' => 'ETHBTC',
                              'id' => 789,
                              'orderId' => 101,
                              'price' => '0.05',
                              'qty' => '10.0',
                              'quoteQty' => '0.5',
                              'commission' => '0.001',
                              'commissionAsset' => 'ETH',
                              'time' => 1_710_936_000_000,
                              'isBuyer' => true
                            }
                          ])
    )
    honeymaker_client.stubs(:deposit_history).returns(Result::Success.new([]))
    honeymaker_client.stubs(:withdraw_history).returns(Result::Success.new([]))

    eth = create(:asset, :ethereum)
    btc = create(:asset, :bitcoin)
    create(:ticker, exchange: @exchange, base_asset: eth, quote_asset: btc,
                    base: 'ETH', quote: 'BTC', ticker: 'ETHBTC',
                    minimum_base_size: 0.001, minimum_quote_size: 0.0001,
                    base_decimals: 8, quote_decimals: 8, price_decimals: 6)

    @exchange.stubs(:tickers).returns(Ticker.where(exchange: @exchange))

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    entries = result.data
    swap_entries = entries.select { |e| e[:group_id].present? }
    assert_equal 2, swap_entries.size

    swap_in = swap_entries.find { |e| e[:entry_type] == :swap_in }
    swap_out = swap_entries.find { |e| e[:entry_type] == :swap_out }
    assert_equal swap_in[:group_id], swap_out[:group_id]
    assert_equal 'ETH', swap_in[:base_currency]
    assert_equal 'BTC', swap_out[:base_currency]
  end

  test 'returns failure when honeymaker client fails' do
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    honeymaker_client.stubs(:deposit_history).returns(Result::Failure.new('API rate limit'))

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.failure?
  end

  test 'passes start_time as milliseconds to honeymaker' do
    start_time = Time.utc(2026, 3, 20)
    honeymaker_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(honeymaker_client)

    expected_ms = (start_time.to_f * 1000).to_i
    honeymaker_client.expects(:deposit_history).with(has_entry(start_time: expected_ms)).returns(Result::Success.new([]))
    honeymaker_client.expects(:withdraw_history).with(has_entry(start_time: expected_ms)).returns(Result::Success.new([]))
    honeymaker_client.stubs(:account_trade_list).returns(Result::Success.new([]))

    @exchange.stubs(:tickers).returns(Ticker.none)

    @exchange.get_ledger(api_key: @api_key, start_time: start_time)
  end
end
