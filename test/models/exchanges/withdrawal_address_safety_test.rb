require 'test_helper'

class WithdrawalAddressSafetyTest < ActiveSupport::TestCase
  # ── supports_withdrawal? returns false for unsupported exchanges ──────────

  %w[Coinbase Kucoin Bitget Bybit Bingx Bitvavo Bitrue].each do |exchange_class|
    test "#{exchange_class} does not support withdrawal" do
      exchange = "Exchanges::#{exchange_class}".constantize.new
      refute_predicate exchange, :supports_withdrawal?
    end
  end

  # ── supports_withdrawal? returns true for supported exchanges ─────────────

  %w[Kraken Binance BinanceUs Bitmart Gemini Mexc].each do |exchange_class|
    test "#{exchange_class} supports withdrawal" do
      exchange = "Exchanges::#{exchange_class}".constantize.new
      assert_predicate exchange, :supports_withdrawal?
    end
  end

  # ── Binance list_withdrawal_addresses ─────────────────────────────────────

  test 'Binance list_withdrawal_addresses parses response correctly' do
    exchange = create_exchange(:binance)
    api_key = mock_api_key(exchange)
    exchange.set_client(api_key: api_key)

    response = [
      { 'coin' => 'BTC', 'address' => '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa', 'name' => 'My BTC Wallet',
        'network' => 'BTC', 'addressTag' => '', 'whiteStatus' => true },
      { 'coin' => 'BTC', 'address' => 'bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh', 'name' => 'SegWit Wallet',
        'network' => 'BTC', 'addressTag' => '', 'whiteStatus' => true }
    ]
    exchange.stubs(:client).returns(stub(get_withdraw_addresses: Result::Success.new(response)))

    asset = stub(id: 1)
    exchange.stubs(:symbol_from_asset).with(asset).returns('BTC')

    addresses = exchange.list_withdrawal_addresses(asset: asset)

    assert_equal 2, addresses.size
    assert_equal '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa', addresses.first[:name]
    assert_includes addresses.first[:label], 'My BTC Wallet'
    assert_includes addresses.first[:label], 'BTC'
  end

  test 'Binance list_withdrawal_addresses returns nil on API failure' do
    exchange = create_exchange(:binance)
    api_key = mock_api_key(exchange)
    exchange.set_client(api_key: api_key)

    exchange.stubs(:client).returns(stub(get_withdraw_addresses: Result::Failure.new('API error')))
    exchange.stubs(:symbol_from_asset).returns('BTC')

    assert_nil exchange.list_withdrawal_addresses(asset: stub(id: 1))
  end

  test 'Binance list_withdrawal_addresses returns nil for unknown symbol' do
    exchange = create_exchange(:binance)
    api_key = mock_api_key(exchange)
    exchange.set_client(api_key: api_key)
    exchange.stubs(:symbol_from_asset).returns(nil)

    assert_nil exchange.list_withdrawal_addresses(asset: stub(id: 1))
  end

  # ── BinanceUs inherits list_withdrawal_addresses from Binance ─────────────

  test 'BinanceUs inherits list_withdrawal_addresses from Binance' do
    assert Exchanges::BinanceUs.instance_method(:list_withdrawal_addresses).owner == Exchanges::Binance
  end

  # ── Bitmart list_withdrawal_addresses ─────────────────────────────────────

  test 'Bitmart list_withdrawal_addresses filters by asset symbol' do
    exchange = create_exchange(:bitmart)
    api_key = mock_api_key(exchange)
    exchange.set_client(api_key: api_key)

    response = {
      'code' => 1000,
      'data' => {
        'withdrawAddressList' => [
          { 'currency' => 'BTC', 'address' => '1abc123', 'network' => 'BTC', 'memo' => '' },
          { 'currency' => 'ETH', 'address' => '0xdef456', 'network' => 'ERC20', 'memo' => '' },
          { 'currency' => 'BTC', 'address' => '3xyz789', 'network' => 'BTC', 'memo' => '' }
        ]
      }
    }
    exchange.stubs(:client).returns(stub(get_withdraw_addresses: Result::Success.new(response)))

    asset = stub(id: 1)
    exchange.stubs(:symbol_from_asset).with(asset).returns('BTC')

    addresses = exchange.list_withdrawal_addresses(asset: asset)

    assert_equal 2, addresses.size
    assert_equal '1abc123', addresses.first[:name]
    assert_includes addresses.first[:label], 'BTC'
  end

  test 'Bitmart list_withdrawal_addresses returns nil on non-1000 code' do
    exchange = create_exchange(:bitmart)
    api_key = mock_api_key(exchange)
    exchange.set_client(api_key: api_key)

    response = { 'code' => 50_001, 'message' => 'API key error' }
    exchange.stubs(:client).returns(stub(get_withdraw_addresses: Result::Success.new(response)))
    exchange.stubs(:symbol_from_asset).returns('BTC')

    assert_nil exchange.list_withdrawal_addresses(asset: stub(id: 1))
  end

  # ── Gemini list_withdrawal_addresses ──────────────────────────────────────

  test 'Gemini list_withdrawal_addresses parses active addresses' do
    exchange = create_exchange(:gemini)
    api_key = mock_api_key(exchange)
    exchange.set_client(api_key: api_key)

    response = {
      'approvedAddresses' => [
        { 'network' => 'bitcoin', 'scope' => 'account', 'label' => 'My BTC',
          'status' => 'active', 'createdAt' => 1_234_567_890,
          'address' => '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa' },
        { 'network' => 'bitcoin', 'scope' => 'account', 'label' => 'Pending Wallet',
          'status' => 'pending', 'createdAt' => 1_234_567_891,
          'address' => 'bc1qpending' }
      ]
    }
    exchange.stubs(:client).returns(stub(get_approved_addresses: Result::Success.new(response)))

    asset = stub(id: 1)
    exchange.stubs(:symbol_from_asset).with(asset).returns('BTC')

    addresses = exchange.list_withdrawal_addresses(asset: asset)

    assert_equal 1, addresses.size
    assert_equal '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa', addresses.first[:name]
    assert_includes addresses.first[:label], 'My BTC'
  end

  test 'Gemini list_withdrawal_addresses maps symbols to network names' do
    exchange = create_exchange(:gemini)
    api_key = mock_api_key(exchange)
    exchange.set_client(api_key: api_key)

    response = { 'approvedAddresses' => [] }
    client = stub
    client.stubs(:get_approved_addresses).with(network: 'ethereum').returns(Result::Success.new(response))
    exchange.stubs(:client).returns(client)
    exchange.stubs(:symbol_from_asset).returns('ETH')

    addresses = exchange.list_withdrawal_addresses(asset: stub(id: 1))

    assert_equal [], addresses
  end

  test 'Gemini list_withdrawal_addresses returns nil for unmapped symbol' do
    exchange = create_exchange(:gemini)
    api_key = mock_api_key(exchange)
    exchange.set_client(api_key: api_key)
    exchange.stubs(:symbol_from_asset).returns('OBSCURECOIN')

    assert_nil exchange.list_withdrawal_addresses(asset: stub(id: 1))
  end

  # ── MEXC list_withdrawal_addresses ────────────────────────────────────────

  test 'MEXC list_withdrawal_addresses filters by asset symbol' do
    exchange = create_exchange(:mexc)
    api_key = mock_api_key(exchange)
    exchange.set_client(api_key: api_key)

    response = [
      { 'coin' => 'BTC', 'address' => '1btc123', 'network' => 'BTC', 'memo' => '' },
      { 'coin' => 'ETH', 'address' => '0xeth456', 'network' => 'ERC20', 'memo' => '' }
    ]
    exchange.stubs(:client).returns(stub(get_withdraw_addresses: Result::Success.new(response)))

    asset = stub(id: 1)
    exchange.stubs(:symbol_from_asset).with(asset).returns('BTC')

    addresses = exchange.list_withdrawal_addresses(asset: asset)

    assert_equal 1, addresses.size
    assert_equal '1btc123', addresses.first[:name]
  end

  test 'MEXC list_withdrawal_addresses returns nil on API failure' do
    exchange = create_exchange(:mexc)
    api_key = mock_api_key(exchange)
    exchange.set_client(api_key: api_key)

    exchange.stubs(:client).returns(stub(get_withdraw_addresses: Result::Failure.new('error')))
    exchange.stubs(:symbol_from_asset).returns('BTC')

    assert_nil exchange.list_withdrawal_addresses(asset: stub(id: 1))
  end

  private

  def create_exchange(type)
    case type
    when :binance then Exchanges::Binance.find_or_create_by!(name: 'Binance', available: true)
    when :bitmart then Exchanges::Bitmart.find_or_create_by!(name: 'BitMart', available: true)
    when :gemini  then Exchanges::Gemini.find_or_create_by!(name: 'Gemini', available: true)
    when :mexc    then Exchanges::Mexc.find_or_create_by!(name: 'MEXC', available: true)
    end
  end

  def mock_api_key(exchange)
    stub(key: 'test_key', secret: 'test_secret', passphrase: 'test_memo', correct?: true,
         exchange: exchange, key_type: :withdrawal, withdrawal?: true)
  end
end
