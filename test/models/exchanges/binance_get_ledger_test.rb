require 'test_helper'

class Exchanges::BinanceGetLedgerTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
  end

  def stub_common(hm_client, extra_coins: [])
    Honeymaker.stubs(:client).returns(hm_client)
    hm_client.stubs(:deposit_history).returns(Result::Success.new([]))
    hm_client.stubs(:withdraw_history).returns(Result::Success.new([]))
    hm_client.stubs(:account_trade_list).returns(Result::Success.new([]))
    hm_client.stubs(:convert_trade_flow).returns(Result::Success.new({ 'list' => [] }))
    hm_client.stubs(:fiat_payments).returns(Result::Success.new({ 'data' => [] }))
    hm_client.stubs(:dust_log).returns(Result::Success.new({ 'userAssetDribblets' => [] }))
    hm_client.stubs(:asset_dividend).returns(Result::Success.new({ 'rows' => [] }))
    hm_client.stubs(:simple_earn_flexible_rewards).returns(Result::Success.new({ 'rows' => [], 'total' => 0 }))
    hm_client.stubs(:simple_earn_locked_rewards).returns(Result::Success.new({ 'rows' => [], 'total' => 0 }))
    hm_client.stubs(:margin_interest_history).returns(Result::Success.new({ 'rows' => [] }))
    hm_client.stubs(:margin_force_liquidation).returns(Result::Success.new({ 'rows' => [] }))
    hm_client.stubs(:futures_income_history).returns(Result::Success.new([]))
    hm_client.stubs(:coin_futures_income_history).returns(Result::Success.new([]))
    hm_client.stubs(:simple_earn_flexible_subscriptions).returns(Result::Success.new({ 'rows' => [], 'total' => 0 }))
    hm_client.stubs(:simple_earn_flexible_redemptions).returns(Result::Success.new({ 'rows' => [], 'total' => 0 }))
    hm_client.stubs(:simple_earn_locked_subscriptions).returns(Result::Success.new({ 'rows' => [], 'total' => 0 }))
    hm_client.stubs(:simple_earn_locked_redemptions).returns(Result::Success.new({ 'rows' => [], 'total' => 0 }))

    balances = extra_coins.map { |c| { 'asset' => c, 'free' => '1.0', 'locked' => '0' } }
    hm_client.stubs(:account_information).returns(Result::Success.new({ 'balances' => balances }))
    hm_client.stubs(:exchange_information).returns(Result::Success.new({
                                                                         'symbols' => [
                                                                           { 'symbol' => 'BTCUSDT', 'baseAsset' => 'BTC', 'quoteAsset' => 'USDT' },
                                                                           { 'symbol' => 'ETHUSDT', 'baseAsset' => 'ETH', 'quoteAsset' => 'USDT' },
                                                                           { 'symbol' => 'ETHBTC', 'baseAsset' => 'ETH', 'quoteAsset' => 'BTC' }
                                                                         ]
                                                                       }))
  end

  test 'returns normalized trade entries from account_trade_list' do
    hm_client = mock('honeymaker_client')
    stub_common(hm_client, extra_coins: %w[BTC])

    hm_client.stubs(:account_trade_list).with(has_entry(symbol: 'BTCUSDT')).returns(
      Result::Success.new([{
                            'symbol' => 'BTCUSDT', 'id' => 123, 'orderId' => 456,
                            'price' => '50000.00', 'qty' => '0.5', 'quoteQty' => '25000.00',
                            'commission' => '25.00', 'commissionAsset' => 'USDT',
                            'time' => 1_710_936_000_000, 'isBuyer' => true
                          }])
    )

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    trade = result.data.find { |e| e[:entry_type] == :buy }
    assert_not_nil trade
    assert_equal 'BTC', trade[:base_currency]
    assert_equal 0.5, trade[:base_amount]
    assert_equal 'USDT', trade[:quote_currency]
    assert_equal 25_000.0, trade[:quote_amount]
    assert_equal 'USDT', trade[:fee_currency]
    assert_equal 25.0, trade[:fee_amount]
  end

  test 'returns normalized deposit entries' do
    hm_client = mock('honeymaker_client')
    stub_common(hm_client)

    hm_client.stubs(:deposit_history).returns(
      Result::Success.new([{
                            'coin' => 'BTC', 'amount' => '1.0', 'status' => 1,
                            'txId' => '0xabc123', 'insertTime' => 1_710_936_000_000
                          }])
    )

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    deposit = result.data.find { |e| e[:entry_type] == :deposit }
    assert_not_nil deposit
    assert_equal 'BTC', deposit[:base_currency]
    assert_equal 1.0, deposit[:base_amount]
    assert_equal '0xabc123', deposit[:tx_id]
  end

  test 'returns normalized withdrawal entries' do
    hm_client = mock('honeymaker_client')
    stub_common(hm_client)

    hm_client.stubs(:withdraw_history).returns(
      Result::Success.new([{
                            'coin' => 'ETH', 'amount' => '2.0', 'transactionFee' => '0.005',
                            'status' => 6, 'txId' => '0xdef456',
                            'applyTime' => '2026-03-20 10:00:00', 'id' => 'w-1'
                          }])
    )

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
    hm_client = mock('honeymaker_client')
    stub_common(hm_client, extra_coins: %w[ETH])

    hm_client.stubs(:account_trade_list).with(has_entry(symbol: 'ETHBTC')).returns(
      Result::Success.new([{
                            'symbol' => 'ETHBTC', 'id' => 789, 'orderId' => 101,
                            'price' => '0.05', 'qty' => '10.0', 'quoteQty' => '0.5',
                            'commission' => '0.001', 'commissionAsset' => 'ETH',
                            'time' => 1_710_936_000_000, 'isBuyer' => true
                          }])
    )

    result = @exchange.get_ledger(api_key: @api_key)

    assert result.success?
    swap_entries = result.data.select { |e| e[:group_id].present? }
    assert_equal 2, swap_entries.size
    assert_equal swap_entries[0][:group_id], swap_entries[1][:group_id]
    assert_equal 'ETH', swap_entries.find { |e| e[:entry_type] == :swap_in }[:base_currency]
    assert_equal 'BTC', swap_entries.find { |e| e[:entry_type] == :swap_out }[:base_currency]
  end

  test 'returns failure when deposit_history fails' do
    hm_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(hm_client)
    hm_client.stubs(:deposit_history).returns(Result::Failure.new('API rate limit'))

    result = @exchange.get_ledger(api_key: @api_key)
    assert result.failure?
  end

  test 'passes start_time as milliseconds' do
    start_time = Time.utc(2026, 3, 20)
    hm_client = mock('honeymaker_client')
    stub_common(hm_client)

    expected_ms = (start_time.to_f * 1000).to_i
    hm_client.expects(:deposit_history).with(has_entry(start_time: expected_ms)).returns(Result::Success.new([]))
    hm_client.expects(:withdraw_history).with(has_entry(start_time: expected_ms)).returns(Result::Success.new([]))

    @exchange.get_ledger(api_key: @api_key, start_time: start_time)
  end
end
