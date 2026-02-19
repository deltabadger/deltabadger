require 'test_helper'

class Clients::HyperliquidTest < ActiveSupport::TestCase
  setup do
    @wallet = '0x1234567890abcdef1234567890abcdef12345678'
    @agent_key = "0x#{'ab' * 32}"
    @client = Clients::Hyperliquid.new(wallet_address: @wallet, agent_key: @agent_key)
  end

  test 'initializes with wallet_address and agent_key' do
    assert_kind_of Clients::Hyperliquid, @client
  end

  test 'initializes without credentials for read-only operations' do
    client = Clients::Hyperliquid.new
    assert_kind_of Clients::Hyperliquid, client
  end

  test 'spot_meta wraps gem call and returns Result' do
    mock_info = mock('info')
    mock_info.expects(:spot_meta).returns({ 'tokens' => [], 'universe' => [] })
    @client.instance_variable_set(:@info, mock_info)

    result = @client.spot_meta
    assert result.success?
    assert_equal({ 'tokens' => [], 'universe' => [] }, result.data)
  end

  test 'all_mids wraps gem call and returns Result' do
    mock_info = mock('info')
    mock_info.expects(:all_mids).returns({ 'ETH' => '2500.00' })
    @client.instance_variable_set(:@info, mock_info)

    result = @client.all_mids
    assert result.success?
    assert_equal({ 'ETH' => '2500.00' }, result.data)
  end

  test 'spot_balances wraps gem call and returns Result' do
    mock_info = mock('info')
    mock_info.expects(:spot_balances).with(@wallet).returns({ 'balances' => [] })
    @client.instance_variable_set(:@info, mock_info)

    result = @client.spot_balances
    assert result.success?
  end

  test 'order wraps gem call and returns Result' do
    mock_exchange = mock('exchange')
    mock_exchange.expects(:order).with(
      coin: 'PURR/USDC',
      is_buy: true,
      size: '10',
      limit_px: '2.50',
      order_type: { limit: { tif: 'Gtc' } }
    ).returns({ 'status' => 'ok' })
    @client.instance_variable_set(:@exchange, mock_exchange)

    result = @client.order(
      coin: 'PURR/USDC',
      is_buy: true,
      size: '10',
      limit_px: '2.50'
    )
    assert result.success?
  end

  test 'cancel wraps gem call and returns Result' do
    mock_exchange = mock('exchange')
    mock_exchange.expects(:cancel).with(coin: 'PURR/USDC', oid: 123_456).returns({ 'status' => 'ok' })
    @client.instance_variable_set(:@exchange, mock_exchange)

    result = @client.cancel(coin: 'PURR/USDC', oid: 123_456)
    assert result.success?
  end

  test 'order_status wraps gem call and returns Result' do
    mock_info = mock('info')
    mock_info.expects(:order_status).with(@wallet, 123_456).returns({ 'status' => 'filled' })
    @client.instance_variable_set(:@info, mock_info)

    result = @client.order_status(oid: 123_456)
    assert result.success?
  end

  test 'l2_book wraps gem call and returns Result' do
    mock_info = mock('info')
    mock_info.expects(:l2_book).with('PURR/USDC').returns({ 'levels' => [[], []] })
    @client.instance_variable_set(:@info, mock_info)

    result = @client.l2_book(coin: 'PURR/USDC')
    assert result.success?
  end

  test 'candles_snapshot wraps gem call and returns Result' do
    mock_info = mock('info')
    mock_info.expects(:candles_snapshot).with('PURR/USDC', '1h', 1_700_000_000_000, 1_700_086_400_000).returns([])
    @client.instance_variable_set(:@info, mock_info)

    result = @client.candles_snapshot(
      coin: 'PURR/USDC',
      interval: '1h',
      start_time: 1_700_000_000_000,
      end_time: 1_700_086_400_000
    )
    assert result.success?
  end

  test 'with_rescue catches Hyperliquid::Error and returns Failure' do
    mock_info = mock('info')
    mock_info.expects(:spot_meta).raises(Hyperliquid::ClientError.new('Bad request'))
    @client.instance_variable_set(:@info, mock_info)

    result = @client.spot_meta
    assert result.failure?
    assert_match(/Bad request/, result.errors.first)
  end

  test 'with_rescue catches Hyperliquid::ServerError and returns Failure' do
    mock_info = mock('info')
    mock_info.expects(:spot_meta).raises(Hyperliquid::ServerError.new('Internal error'))
    @client.instance_variable_set(:@info, mock_info)

    result = @client.spot_meta
    assert result.failure?
    assert_match(/Internal error/, result.errors.first)
  end
end
